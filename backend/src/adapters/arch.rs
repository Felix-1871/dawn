// SPDX-License-Identifier: GPL-3.0-or-later

//! The `arch` adapter: LuminOS's distro-specific pipeline steps. See
//! SPEC.md "LuminOS adapter". A few exact values here (the live user's
//! name, the login manager package) aren't nailed down by the spec yet;
//! they're marked below and are expected to be confirmed against a real
//! LuminOS ISO in M1, not treated as verified fact.

use plan::{InstallPlan, Source};

use super::{Adapter, DiskLayout};
use crate::runner::{Action, Invocation};

/// Path to the archiso squashfs image, from `installer.toml`'s
/// `[source] squashfs` key (SPEC.md "Branding & configuration"). Config
/// file loading isn't wired up until later, so M0 hardcodes the example
/// value from the spec.
const SQUASHFS_IMAGE: &str = "/run/archiso/bootmnt/arch/x86_64/airootfs.sfs";

/// Placeholder pending confirmation against the real ISO profile (M1/M5).
const LIVE_USER: &str = "liveuser";

/// Placeholder pending confirmation of which login manager
/// `luminos-desktop` actually depends on (M1/M5).
const LOGIN_MANAGER: &str = "greetd";

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
                vec![Action::Run(Invocation::new("pacstrap", args))]
            }
            Source::Squashfs => vec![Action::Run(
                Invocation::new(
                    "unsquashfs",
                    ["-f", "-d", layout.target.as_str(), SQUASHFS_IMAGE],
                )
                .with_note("kernel is copied from the medium separately if /boot has none"),
            )],
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
            Action::WriteFile {
                path: format!(
                    "{}/etc/systemd/system/getty@tty1.service.d/autologin.conf",
                    layout.target
                ),
                description: "remove (was the live session's autologin drop-in)".to_string(),
            },
            Action::WriteFile {
                path: format!("{}/etc/mkinitcpio.conf.d/archiso.conf", layout.target),
                description: "remove (archiso-only mkinitcpio config)".to_string(),
            },
            Action::Run(Self::chroot(layout, ["pacman-key", "--init"])),
            Action::Run(Self::chroot(
                layout,
                ["pacman-key", "--populate", "archlinux", "luminos"],
            )),
        ]
    }

    fn keyboard_config(&self, plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action> {
        vec![Action::WriteFile {
            path: format!("{}/etc/skel/.config/hypr/hyprland.conf", layout.target),
            description: format!(
                "kb_layout = {}, kb_variant = {}",
                plan.keyboard.layout, plan.keyboard.variant
            ),
        }]
    }

    fn admin_group(&self) -> &'static str {
        "wheel"
    }

    fn build_ukis(&self, _plan: &InstallPlan, layout: &DiskLayout) -> Vec<Action> {
        vec![
            Action::WriteFile {
                path: format!("{}/etc/kernel/cmdline", layout.target),
                description: format!("root={} rw", layout.root_device),
            },
            Action::WriteFile {
                path: format!("{}/etc/mkinitcpio.d/linux.preset", layout.target),
                description: "default_uki and fallback_uki, systemd hooks + microcode".to_string(),
            },
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
            actions.push(Action::WriteFile {
                path: format!("{}/etc/greetd/config.toml", layout.target),
                description: format!("autologin as {}", plan.user.username),
            });
        }
        actions
    }
}
