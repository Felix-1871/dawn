// SPDX-License-Identifier: GPL-3.0-or-later

//! The `arch` adapter: LuminOS's distro-specific pipeline steps. See
//! SPEC.md "LuminOS adapter". What it needs from `installer.toml` (the
//! offline image, what offline cleanup removes, the admin group) and from
//! the live system (whether archiso copied the image to RAM) is fixed
//! when it's built, so building the command list stays a pure function
//! of the plan.

use plan::config::InstallerConfig;
use plan::{InstallPlan, Source};

use super::{Adapter, DiskLayout};
use crate::live::LiveSystem;
use crate::pipeline::PACMAN_CONF;
use crate::runner::{Action, Invocation, ProgressFormat};

/// The LuminOS live ISO's own user account, removed during offline
/// cleanup. Confirmed against the real ISO — see DECISIONS.md.
const LIVE_USER: &str = "luminos";

/// Dawn's own package, which an offline install always removes (SPEC.md
/// `installer.toml` example: "Dawn itself is always removed by the
/// adapter").
const DAWN_PACKAGE: &str = "luminos-dawn";

/// `luminos-desktop`'s login manager: greetd, with regreet (hosted under
/// Hyprland rather than a separate compositor like cage) as the default
/// greeter. greetd is protocol-based, so the greeter can be swapped
/// without touching Dawn; regreet gives a CSS-themeable graphical login
/// that can carry LuminOS branding, and since Hyprland is already a
/// luminos-desktop dependency, hosting regreet under it adds no extra
/// compositor package. The greeter itself is luminos-desktop's default
/// greetd config, not Dawn's; Dawn only enables the service and adds an
/// `[initial_session]` when the plan asks for autologin. See DECISIONS.md.
const LOGIN_MANAGER: &str = "greetd";

/// What greetd starts for an autologin: the desktop session itself. If
/// luminos-desktop starts Hyprland some other way (a wrapper, uwsm),
/// this has to follow — see LUMINOS-CHANGES.md.
const SESSION_COMMAND: &str = "Hyprland";

/// mkinitcpio preset naming the two UKIs systemd-boot will list
/// automatically from `/boot/EFI/Linux/` (SPEC.md "Disk & bootloader":
/// "systemd-boot lists UKIs there automatically, so no loader entries are
/// written"). The HOOKS array that actually makes these UKIs
/// systemd-based (base systemd autodetect microcode ... sd-vconsole ...)
/// is luminos-base's own `/etc/mkinitcpio.conf`, not something Dawn picks
/// per install — see DECISIONS.md. Microcode comes from that `microcode`
/// hook too: mkinitcpio now ignores a preset's `ALL_microcode`, with a
/// deprecation warning pointing at the hook.
fn mkinitcpio_preset() -> String {
    "# mkinitcpio preset for the linux package — built by dawn-backend\n\
     \n\
     ALL_kver=\"/boot/vmlinuz-linux\"\n\
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

pub struct ArchAdapter {
    squashfs_image: String,
    remove_packages: Vec<String>,
    admin_group: String,
}

impl ArchAdapter {
    pub fn new(config: &InstallerConfig, live: &LiveSystem) -> Self {
        Self {
            squashfs_image: live.squashfs_image.clone(),
            remove_packages: config.offline_cleanup.remove_packages.clone(),
            admin_group: config.defaults.admin_group.clone(),
        }
    }

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
                    PACMAN_CONF.to_string(),
                    layout.target.clone(),
                ];
                args.extend(plan.packages.iter().cloned());
                vec![
                    Action::Run(
                        Invocation::new("pacstrap", args).with_progress(ProgressFormat::Pacman),
                    ),
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
                Action::Run(
                    Invocation::new(
                        "unsquashfs",
                        [
                            "-percentage",
                            "-f",
                            "-d",
                            layout.target.as_str(),
                            self.squashfs_image.as_str(),
                        ],
                    )
                    .with_progress(ProgressFormat::Percentage),
                ),
                // archiso moves the kernel out of the image's /boot onto
                // the medium, which copytoram leaves unmounted. The image
                // still has the kernel its linux package installed.
                Action::InstallKernelFromModules {
                    root: layout.target.clone(),
                    pkgbase: "linux".to_string(),
                    destination: format!("{}/boot/vmlinuz-linux", layout.target),
                },
            ],
        }
    }

    fn offline_cleanup(&self, _plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action> {
        let mut remove = vec![
            "pacman".to_string(),
            "-Rns".to_string(),
            "--noconfirm".to_string(),
            DAWN_PACKAGE.to_string(),
        ];
        remove.extend(self.remove_packages.iter().cloned());
        vec![
            Action::Run(Self::chroot(layout, remove)),
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

    fn admin_group(&self) -> &str {
        &self.admin_group
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
            // greetd's `initial_session` runs once, on the first start
            // after boot, without going through the greeter. It's added
            // to luminos-desktop's own config.toml, whose
            // `default_session` (the greeter) handles every later login;
            // greetd has no drop-in directory to put it in instead.
            actions.push(Action::SetTomlTable {
                path: format!("{}/etc/greetd/config.toml", layout.target),
                table: "initial_session".to_string(),
                entries: vec![
                    ("command".to_string(), SESSION_COMMAND.to_string()),
                    ("user".to_string(), plan.user.username.clone()),
                ],
            });
        }
        actions
    }
}
