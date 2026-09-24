// SPDX-License-Identifier: GPL-3.0-or-later

//! Turns what the screens collected into a real [`plan::InstallPlan`].
//! Kept free of the generated Slint types so it's plain, testable Rust:
//! `lib.rs` reads the `AppWindow`'s properties into [`PlanFields`] and
//! hands them here.

use plan::{
    CURRENT_VERSION, Disk, DiskMode, Filesystem, InstallPlan, Keyboard, Redacted, SecureBoot,
    Source, User,
};

/// Everything the screens collected, gathered from `AppWindow`'s
/// properties, plus what `installer.toml` fixes (`packages`).
pub struct PlanFields {
    pub locale: String,
    pub kb_layout: String,
    pub kb_variant: String,
    pub timezone: String,
    pub hostname: String,
    /// Whether the package repositories were reachable, last checked:
    /// at startup, or after joining Wi-Fi. Decides online (pacstrap) or
    /// offline (the live image) install.
    pub online: bool,
    /// The Wi-Fi profile joined on the Network screen, if any. It's set
    /// up on the installed system too, whichever way it's installed.
    pub network_profile: String,
    pub packages: Vec<String>,
    pub disk_device: String,
    pub secure_boot_enroll: bool,
    pub full_name: String,
    pub username: String,
    pub password: String,
    pub autologin: bool,
}

pub fn build_install_plan(fields: &PlanFields) -> InstallPlan {
    InstallPlan {
        version: CURRENT_VERSION,
        locale: fields.locale.clone(),
        keyboard: Keyboard {
            layout: fields.kb_layout.clone(),
            variant: fields.kb_variant.clone(),
        },
        timezone: fields.timezone.clone(),
        hostname: fields.hostname.clone(),
        source: if fields.online {
            Source::Pacstrap
        } else {
            Source::Squashfs
        },
        packages: fields.packages.clone(),
        network_profile: (!fields.network_profile.is_empty())
            .then(|| fields.network_profile.clone()),
        // Manual mode needs partition assignments this screen doesn't
        // collect yet (M7 scope, SPEC.md milestones) — the Disk
        // screen's Manual toggle stays disabled until then, so Erase
        // is the only mode a plan built here can ever be.
        disk: Disk {
            mode: DiskMode::Erase,
            device: fields.disk_device.clone(),
            filesystem: Filesystem::Btrfs,
            partitions: None,
        },
        secure_boot: SecureBoot {
            enroll: fields.secure_boot_enroll,
            microsoft_keys: true,
        },
        user: User {
            full_name: fields.full_name.clone(),
            username: fields.username.clone(),
            password: Redacted::new(fields.password.clone()),
            autologin: fields.autologin,
        },
    }
}

/// The first word of the full name, lowercased down to what
/// [`plan::validate::is_valid_username`] accepts. Empty if nothing
/// usable is left (an emoji-only name, say) — the Account screen's
/// Next button already requires a non-empty username before
/// continuing, so this is a starting point for the user to edit, not a
/// guarantee.
pub fn derive_username(full_name: &str) -> String {
    full_name
        .split_whitespace()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// The username doubles as the default hostname; both are editable
/// afterward (SPEC.md's Account screen notes).
pub fn derive_hostname(username: &str) -> String {
    username.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_fields() -> PlanFields {
        PlanFields {
            locale: "en_US.UTF-8".into(),
            kb_layout: "us".into(),
            kb_variant: "".into(),
            timezone: "Europe/Berlin".into(),
            hostname: "ada-laptop".into(),
            online: true,
            network_profile: "".into(),
            packages: vec!["luminos-base".into(), "luminos-desktop".into()],
            disk_device: "/dev/disk/by-id/nvme-EXAMPLE_SERIAL".into(),
            secure_boot_enroll: false,
            full_name: "Ada Lovelace".into(),
            username: "ada".into(),
            password: "correct-horse-battery-staple".into(),
            autologin: false,
        }
    }

    #[test]
    fn builds_an_online_plan_with_no_network_profile() {
        let plan = build_install_plan(&sample_fields());
        assert_eq!(plan.source, Source::Pacstrap);
        assert_eq!(plan.network_profile, None);
    }

    #[test]
    fn joining_wifi_carries_the_profile_into_the_plan() {
        let mut fields = sample_fields();
        fields.network_profile = "home-wifi".into();
        let plan = build_install_plan(&fields);
        assert_eq!(plan.source, Source::Pacstrap);
        assert_eq!(plan.network_profile, Some("home-wifi".to_string()));
    }

    #[test]
    fn staying_offline_installs_from_the_live_image() {
        let mut fields = sample_fields();
        fields.online = false;
        let plan = build_install_plan(&fields);
        assert_eq!(plan.source, Source::Squashfs);
        assert_eq!(plan.network_profile, None);
    }

    #[test]
    fn derives_a_lowercase_username_from_the_first_name() {
        assert_eq!(derive_username("Ada Lovelace"), "ada");
        assert_eq!(derive_username("grace hopper"), "grace");
    }

    #[test]
    fn derived_username_is_valid() {
        assert!(plan::validate::is_valid_username(&derive_username(
            "Ada Lovelace"
        )));
    }

    #[test]
    fn hostname_defaults_to_the_username() {
        assert_eq!(derive_hostname("ada"), "ada");
    }
}
