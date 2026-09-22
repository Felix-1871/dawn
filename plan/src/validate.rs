// SPDX-License-Identifier: GPL-3.0-or-later

//! Plan validation. Pure functions over [`InstallPlan`] — no I/O, no disk
//! probing (the backend re-probes the real disk separately; see SPEC.md
//! pipeline step 1). Collects every problem found rather than stopping at
//! the first, so a caller can show the user the whole list at once.

use thiserror::Error;

use crate::install_plan::{CURRENT_VERSION, Disk, DiskMode, InstallPlan, PartitionRole, Source};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ValidationError {
    #[error("plan version {found} is not supported (expected {expected})")]
    UnsupportedVersion { found: u32, expected: u32 },

    #[error("locale must not be empty")]
    EmptyLocale,

    #[error("keyboard layout must not be empty")]
    EmptyKeyboardLayout,

    #[error("timezone {0:?} is not in Region/City form")]
    InvalidTimezone(String),

    #[error("hostname {0:?} is not a valid hostname label")]
    InvalidHostname(String),

    #[error("username {0:?} is not a valid Linux username")]
    InvalidUsername(String),

    #[error("full name must not be empty")]
    EmptyFullName,

    #[error("password must not be empty")]
    EmptyPassword,

    #[error("pacstrap installs need at least one package")]
    NoPackagesForPacstrap,

    #[error("disk device {0:?} must be a /dev/disk/by-id/ path, not a raw device node")]
    DeviceNotById(String),

    #[error("erase mode does not take a partitions list; it writes a fixed layout")]
    PartitionsInEraseMode,

    #[error("manual mode needs a partitions list")]
    MissingPartitionsInManualMode,

    #[error("manual mode needs exactly one ESP partition, found {0}")]
    ExpectedOneEsp(usize),

    #[error("manual mode needs exactly one root partition, found {0}")]
    ExpectedOneRoot(usize),

    #[error("partition mount point {0:?} is not an absolute path")]
    InvalidMountPoint(String),

    #[error("mount point {0:?} is assigned to more than one partition")]
    DuplicateMountPoint(String),

    #[error("secure_boot.enroll requires secure_boot.microsoft_keys (v1 always keeps them)")]
    SecureBootMustKeepMicrosoftKeys,
}

pub fn validate(plan: &InstallPlan) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    if plan.version != CURRENT_VERSION {
        errors.push(ValidationError::UnsupportedVersion {
            found: plan.version,
            expected: CURRENT_VERSION,
        });
    }

    if plan.locale.trim().is_empty() {
        errors.push(ValidationError::EmptyLocale);
    }

    if plan.keyboard.layout.trim().is_empty() {
        errors.push(ValidationError::EmptyKeyboardLayout);
    }

    if !is_valid_timezone(&plan.timezone) {
        errors.push(ValidationError::InvalidTimezone(plan.timezone.clone()));
    }

    if !is_valid_hostname(&plan.hostname) {
        errors.push(ValidationError::InvalidHostname(plan.hostname.clone()));
    }

    if plan.source == Source::Pacstrap && plan.packages.is_empty() {
        errors.push(ValidationError::NoPackagesForPacstrap);
    }

    validate_disk(&plan.disk, &mut errors);

    if plan.secure_boot.enroll && !plan.secure_boot.microsoft_keys {
        errors.push(ValidationError::SecureBootMustKeepMicrosoftKeys);
    }

    if plan.user.full_name.trim().is_empty() {
        errors.push(ValidationError::EmptyFullName);
    }

    if !is_valid_username(&plan.user.username) {
        errors.push(ValidationError::InvalidUsername(plan.user.username.clone()));
    }

    if plan.user.password.expose().is_empty() {
        errors.push(ValidationError::EmptyPassword);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_disk(disk: &Disk, errors: &mut Vec<ValidationError>) {
    if !disk.device.starts_with("/dev/disk/by-id/") {
        errors.push(ValidationError::DeviceNotById(disk.device.clone()));
    }

    match disk.mode {
        DiskMode::Erase => {
            if disk.partitions.is_some() {
                errors.push(ValidationError::PartitionsInEraseMode);
            }
        }
        DiskMode::Manual => {
            let Some(partitions) = &disk.partitions else {
                errors.push(ValidationError::MissingPartitionsInManualMode);
                return;
            };

            let esp_count = partitions
                .iter()
                .filter(|p| p.role == PartitionRole::Esp)
                .count();
            if esp_count != 1 {
                errors.push(ValidationError::ExpectedOneEsp(esp_count));
            }

            let root_count = partitions
                .iter()
                .filter(|p| p.role == PartitionRole::Root)
                .count();
            if root_count != 1 {
                errors.push(ValidationError::ExpectedOneRoot(root_count));
            }

            let mut seen_mount_points = Vec::new();
            for partition in partitions {
                if !partition.mount_point.starts_with('/') {
                    errors.push(ValidationError::InvalidMountPoint(
                        partition.mount_point.clone(),
                    ));
                    continue;
                }
                if seen_mount_points.contains(&partition.mount_point) {
                    errors.push(ValidationError::DuplicateMountPoint(
                        partition.mount_point.clone(),
                    ));
                } else {
                    seen_mount_points.push(partition.mount_point.clone());
                }
            }
        }
    }
}

/// A conservative subset of `useradd`'s NAME_REGEX: starts with a lowercase
/// letter or underscore, then lowercase letters, digits, underscores or
/// hyphens, at most 32 characters.
pub fn is_valid_username(username: &str) -> bool {
    if username.is_empty() || username.len() > 32 {
        return false;
    }
    let mut chars = username.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_lowercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// RFC 1123 hostname label: 1-63 characters, alphanumeric or hyphen, and it
/// doesn't start or end with a hyphen.
pub fn is_valid_hostname(hostname: &str) -> bool {
    if hostname.is_empty() || hostname.len() > 63 {
        return false;
    }
    if hostname.starts_with('-') || hostname.ends_with('-') {
        return false;
    }
    hostname
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn is_valid_timezone(timezone: &str) -> bool {
    let Some((region, city)) = timezone.split_once('/') else {
        return false;
    };
    !region.is_empty() && !city.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install_plan::{Filesystem, Keyboard, PartitionAssignment, SecureBoot, User};
    use crate::redacted::Redacted;

    fn valid_plan() -> InstallPlan {
        InstallPlan {
            version: CURRENT_VERSION,
            locale: "en_US.UTF-8".into(),
            keyboard: Keyboard {
                layout: "us".into(),
                variant: String::new(),
            },
            timezone: "Europe/Berlin".into(),
            hostname: "ada-laptop".into(),
            source: Source::Pacstrap,
            packages: vec!["luminos-base".into(), "luminos-desktop".into()],
            network_profile: None,
            disk: Disk {
                mode: DiskMode::Erase,
                device: "/dev/disk/by-id/nvme-EXAMPLE_SERIAL".into(),
                filesystem: Filesystem::Btrfs,
                partitions: None,
            },
            secure_boot: SecureBoot {
                enroll: false,
                microsoft_keys: true,
            },
            user: User {
                full_name: "Ada Lovelace".into(),
                username: "ada".into(),
                password: Redacted::new("hunter2"),
                autologin: false,
            },
        }
    }

    #[test]
    fn accepts_a_well_formed_plan() {
        assert_eq!(validate(&valid_plan()), Ok(()));
    }

    #[test]
    fn rejects_an_unsupported_version() {
        let mut plan = valid_plan();
        plan.version = 99;
        let errors = validate(&plan).unwrap_err();
        assert!(errors.contains(&ValidationError::UnsupportedVersion {
            found: 99,
            expected: CURRENT_VERSION,
        }));
    }

    #[test]
    fn rejects_a_raw_device_path() {
        let mut plan = valid_plan();
        plan.disk.device = "/dev/nvme0n1".into();
        let errors = validate(&plan).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::DeviceNotById(_)))
        );
    }

    #[test]
    fn rejects_partitions_in_erase_mode() {
        let mut plan = valid_plan();
        plan.disk.partitions = Some(vec![]);
        let errors = validate(&plan).unwrap_err();
        assert!(errors.contains(&ValidationError::PartitionsInEraseMode));
    }

    #[test]
    fn manual_mode_needs_exactly_one_esp_and_one_root() {
        let mut plan = valid_plan();
        plan.disk.mode = DiskMode::Manual;
        plan.disk.partitions = Some(vec![PartitionAssignment {
            device: "/dev/disk/by-id/nvme-EXAMPLE_SERIAL-part1".into(),
            mount_point: "/home".into(),
            format: false,
            role: PartitionRole::Other,
        }]);
        let errors = validate(&plan).unwrap_err();
        assert!(errors.contains(&ValidationError::ExpectedOneEsp(0)));
        assert!(errors.contains(&ValidationError::ExpectedOneRoot(0)));
    }

    #[test]
    fn manual_mode_accepts_one_esp_and_one_root() {
        let mut plan = valid_plan();
        plan.disk.mode = DiskMode::Manual;
        plan.disk.partitions = Some(vec![
            PartitionAssignment {
                device: "/dev/disk/by-id/nvme-EXAMPLE_SERIAL-part1".into(),
                mount_point: "/boot".into(),
                format: false,
                role: PartitionRole::Esp,
            },
            PartitionAssignment {
                device: "/dev/disk/by-id/nvme-EXAMPLE_SERIAL-part2".into(),
                mount_point: "/".into(),
                format: true,
                role: PartitionRole::Root,
            },
        ]);
        assert_eq!(validate(&plan), Ok(()));
    }

    #[test]
    fn rejects_duplicate_mount_points() {
        let mut plan = valid_plan();
        plan.disk.mode = DiskMode::Manual;
        plan.disk.partitions = Some(vec![
            PartitionAssignment {
                device: "/dev/disk/by-id/nvme-EXAMPLE_SERIAL-part1".into(),
                mount_point: "/".into(),
                format: false,
                role: PartitionRole::Esp,
            },
            PartitionAssignment {
                device: "/dev/disk/by-id/nvme-EXAMPLE_SERIAL-part2".into(),
                mount_point: "/".into(),
                format: true,
                role: PartitionRole::Root,
            },
        ]);
        let errors = validate(&plan).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::DuplicateMountPoint(m) if m == "/"))
        );
    }

    #[test]
    fn secure_boot_enroll_requires_microsoft_keys() {
        let mut plan = valid_plan();
        plan.secure_boot.enroll = true;
        plan.secure_boot.microsoft_keys = false;
        let errors = validate(&plan).unwrap_err();
        assert!(errors.contains(&ValidationError::SecureBootMustKeepMicrosoftKeys));
    }

    #[test]
    fn pacstrap_needs_at_least_one_package() {
        let mut plan = valid_plan();
        plan.packages.clear();
        let errors = validate(&plan).unwrap_err();
        assert!(errors.contains(&ValidationError::NoPackagesForPacstrap));
    }

    #[test]
    fn squashfs_does_not_need_packages_listed() {
        let mut plan = valid_plan();
        plan.source = Source::Squashfs;
        plan.packages.clear();
        assert_eq!(validate(&plan), Ok(()));
    }

    #[test]
    fn empty_password_is_rejected() {
        let mut plan = valid_plan();
        plan.user.password = Redacted::new("");
        let errors = validate(&plan).unwrap_err();
        assert!(errors.contains(&ValidationError::EmptyPassword));
    }

    #[test]
    fn empty_password_error_does_not_expose_the_plan_password() {
        let mut plan = valid_plan();
        plan.user.password = Redacted::new("");
        let errors = validate(&plan).unwrap_err();
        for error in &errors {
            assert!(!format!("{error}").contains("hunter2"));
        }
    }

    #[test]
    fn hostname_rules() {
        assert!(is_valid_hostname("ada-laptop"));
        assert!(is_valid_hostname("a"));
        assert!(!is_valid_hostname(""));
        assert!(!is_valid_hostname("-leading-hyphen"));
        assert!(!is_valid_hostname("trailing-hyphen-"));
        assert!(!is_valid_hostname("has a space"));
        assert!(!is_valid_hostname(&"a".repeat(64)));
    }

    #[test]
    fn username_rules() {
        assert!(is_valid_username("ada"));
        assert!(is_valid_username("_service"));
        assert!(is_valid_username("ada-lovelace"));
        assert!(!is_valid_username(""));
        assert!(!is_valid_username("Ada"));
        assert!(!is_valid_username("1ada"));
        assert!(!is_valid_username(&"a".repeat(33)));
    }
}
