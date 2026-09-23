// SPDX-License-Identifier: GPL-3.0-or-later

//! The `arch` adapter: LuminOS's distro-specific pipeline steps. See
//! SPEC.md "LuminOS adapter".

use plan::{InstallPlan, Source};

use super::{Adapter, DiskLayout};
use crate::runner::{Action, Invocation};

/// Where the archiso medium keeps its kernel when it isn't in the
/// squashfs image itself (SPEC.md: "copy `vmlinuz-linux` from the medium
/// if the image has no kernel in `/boot`").
const MEDIUM_KERNEL: &str = "/run/archiso/bootmnt/arch/boot/x86_64/vmlinuz-linux";

/// Path to the archiso squashfs image, from `installer.toml`'s
/// `[source] squashfs` key (SPEC.md "Branding & configuration"). Config
/// file loading isn't wired up until later, so M0 hardcodes the example
/// value from the spec.
const SQUASHFS_IMAGE: &str = "/run/archiso/bootmnt/arch/x86_64/airootfs.sfs";

/// The LuminOS live ISO's own user account, removed during offline
/// cleanup. Confirmed against the real ISO — see DECISIONS.md.
const LIVE_USER: &str = "luminos";

/// `luminos-desktop`'s login manager: greetd, with regreet (hosted under
/// Hyprland rather than a separate compositor like cage) as the default
/// greeter. greetd is protocol-based, so the greeter can be swapped
/// without touching Dawn; regreet gives a CSS-themeable graphical login
/// that can carry LuminOS branding, and since Hyprland is already a
/// luminos-desktop dependency, hosting regreet under it adds no extra
/// compositor package. The greeter itself is luminos-desktop's default
/// greetd config, not Dawn's; Dawn only enables the service and
/// overrides the config when the plan asks for autologin. See
/// DECISIONS.md.
const LOGIN_MANAGER: &str = "greetd";

/// mkinitcpio preset naming the two UKIs systemd-boot will list
/// automatically from `/boot/EFI/Linux/` (SPEC.md "Disk & bootloader":
/// "systemd-boot lists UKIs there automatically, so no loader entries are
/// written"). The HOOKS array that actually makes these UKIs
/// systemd-based (base systemd autodetect microcode ... sd-vconsole ...)
/// is luminos-base's own `/etc/mkinitcpio.conf`, not something Dawn picks
/// per install — see DECISIONS.md.
fn mkinitcpio_preset() -> String {
    "# mkinitcpio preset for the linux package — built by dawn-backend\n\
     \n\
     ALL_kver=\"/boot/vmlinuz-linux\"\n\
     ALL_microcode=(/boot/*-ucode.img)\n\
     \n\
     PRESETS=('default' 'fallback')\n\
     \n\
     default_uki=\"/boot/EFI/Linux/arch-linux.efi\"\n\
     default_options=\"--cmdline /etc/kernel/cmdline\"\n\
     \n\
     fallback_uki=\"/boot/EFI/Linux/arch-linux-fallback.efi\"\n\
     fallback_options=\"-S autodetect --cmdline /etc/kernel/cmdline\"\n"
        .to_string()
}

pub struct ArchAdapter;

impl ArchAdapter {
    fn chroot(
        layout: &DiskLayout,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Invocation {
        let mut full_args = vec![layout.target.clone()];
        full_args.extend(args.into_iter().map(Into::into));
        Invocation::new("arch-chroot", full_args)
    }
}

impl Adapter for ArchAdapter {
    fn name(&self) -> &'static str {
        "arch"
    }

    fn install_base(&self, plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action> {
        match plan.source {
            Source::Pacstrap => {
                let mut args = vec![
                    "-K".to_string(),
                    "-C".to_string(),
                    "/etc/pacman.conf".to_string(),
                    layout.target.clone(),
                ];
                args.extend(plan.packages.iter().cloned());
                vec![
                    Action::Run(Invocation::new("pacstrap", args)),
                    // -K's `pacman-key --init` runs outside pacstrap's PID
                    // namespace, so the gpg-agent and keyboxd it starts
                    // for the target's keyring outlive pacstrap and keep
                    // files under the target open, which makes step 12's
                    // unmount fail with "target is busy".
                    Action::Run(Invocation::new(
                        "gpgconf",
                        [
                            "--homedir",
                            &format!("{}/etc/pacman.d/gnupg", layout.target),
                            "--kill",
                            "all",
                        ],
                    )),
                ]
            }
            Source::Squashfs => vec![
                Action::Run(Invocation::new(
                    "unsquashfs",
                    ["-f", "-d", layout.target.as_str(), SQUASHFS_IMAGE],
                )),
                Action::CopyIfMissing {
                    source: MEDIUM_KERNEL.to_string(),
                    destination: format!("{}/boot/vmlinuz-linux", layout.target),
                },
            ],
        }
    }

    fn offline_cleanup(&self, _plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action> {
        vec![
            Action::Run(Self::chroot(
                layout,
                [
                    "pacman",
                    "-Rns",
                    "--noconfirm",
                    "luminos-dawn",
                    "mkinitcpio-archiso",
                ],
            )),
            Action::Run(Self::chroot(layout, ["userdel", "-r", LIVE_USER])),
            Action::RemoveFile {
                path: format!(
                    "{}/etc/systemd/system/getty@tty1.service.d/autologin.conf",
                    layout.target
                ),
            },
            Action::RemoveFile {
                path: format!("{}/etc/mkinitcpio.conf.d/archiso.conf", layout.target),
            },
            Action::Run(Self::chroot(layout, ["pacman-key", "--init"])),
            Action::Run(Self::chroot(
                layout,
                ["pacman-key", "--populate", "archlinux", "luminos"],
            )),
        ]
    }

    fn keyboard_config(&self, plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action> {
        // Writes a dedicated, Dawn-owned file rather than the shared
        // hyprland.conf: luminos-desktop's default config is expected to
        // `source = ~/.config/hypr/keyboard.conf`, so this can be
        // overwritten wholesale without touching keybinds, autostart, or
        // anything else the user or luminos-desktop put in hyprland.conf.
        // See DECISIONS.md.
        vec![Action::WriteFile {
            path: format!("{}/etc/skel/.config/hypr/keyboard.conf", layout.target),
            content: format!(
                "input {{\n    kb_layout = {}\n    kb_variant = {}\n}}\n",
                plan.keyboard.layout, plan.keyboard.variant
            ),
        }]
    }

    fn admin_group(&self) -> &'static str {
        "wheel"
    }

    fn build_ukis(&self, _plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action> {
        let rootflags = layout
            .root_subvolume
            .as_ref()
            .map(|subvolume| format!(" rootflags=subvol={subvolume}"))
            .unwrap_or_default();
        vec![
            Action::WriteFile {
                path: format!("{}/etc/kernel/cmdline", layout.target),
                // A serial console alongside the primary tty is a
                // harmless, common default (most distros ship one for
                // debugging) — with no serial port present, its getty
                // unit just never comes up. It's also what lets
                // tests/e2e/qemu-run.sh watch for a login prompt over
                // the serial console rather than a framebuffer.
                content: format!(
                    "root={}{rootflags} rw console=tty0 console=ttyS0,115200n8\n",
                    layout.root_device
                ),
            },
            Action::WriteFile {
                path: format!("{}/etc/mkinitcpio.d/linux.preset", layout.target),
                content: mkinitcpio_preset(),
            },
            // The presets write the UKIs into this directory, and a
            // freshly formatted ESP is empty.
            Action::Run(Invocation::new(
                "mkdir",
                ["-p", &format!("{}/boot/EFI/Linux", layout.target)],
            )),
            Action::Run(Self::chroot(layout, ["mkinitcpio", "-P"])),
        ]
    }

    fn services(&self, plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action> {
        let mut actions = vec![
            Action::Run(Self::chroot(
                layout,
                ["systemctl", "enable", "NetworkManager"],
            )),
            Action::Run(Self::chroot(layout, ["systemctl", "enable", LOGIN_MANAGER])),
        ];
        if plan.user.autologin {
            // greetd has no drop-in directory, so autologin means owning
            // the whole file: `initial_session` runs once on the first VT
            // without going through the greeter, while `default_session`
            // (regreet, luminos-desktop's normal default) still handles
            // any later login. Only written when autologin is chosen;
            // otherwise luminos-desktop's own config.toml is left alone.
            actions.push(Action::WriteFile {
                path: format!("{}/etc/greetd/config.toml", layout.target),
                content: format!(
                    "[terminal]\nvt = 1\n\n\
                     [default_session]\ncommand = \"regreet\"\nuser = \"greeter\"\n\n\
                     [initial_session]\ncommand = \"Hyprland\"\nuser = \"{}\"\n",
                    plan.user.username
                ),
            });
        }
        actions
    }
}
