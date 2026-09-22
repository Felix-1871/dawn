// SPDX-License-Identifier: GPL-3.0-or-later

//! The non-negotiable checks from CLAUDE.md's safety rules: a real run
//! needs `--target` to match the plan's disk exactly, and refuses any
//! disk that's mounted or holds the running system. Pure functions here
//! so they're testable without real disks; `main.rs` wires them to
//! `/proc/mounts` and `std::fs::canonicalize`.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SafetyError {
    #[error("--target {target:?} does not match the plan's disk {plan_device:?}")]
    TargetMismatch { target: String, plan_device: String },
    #[error("{device} has mounted partition(s): {}", .mounted.join(", "))]
    DiskIsMounted {
        device: String,
        mounted: Vec<String>,
    },
    #[error("{device} holds the running system's root filesystem")]
    DiskHoldsRunningSystem { device: String },
}

/// The plan names a disk by its `/dev/disk/by-id/...` path; `--target`
/// must repeat that path exactly, so a plan built for one disk can never
/// silently run against another.
pub fn check_target(plan_device: &str, target: &str) -> Result<(), SafetyError> {
    if plan_device == target {
        Ok(())
    } else {
        Err(SafetyError::TargetMismatch {
            target: target.to_string(),
            plan_device: plan_device.to_string(),
        })
    }
}

/// Whether `mount_source` (a device field from `/proc/mounts`, or the
/// root filesystem's source) is the disk named by `canonical_disk` (e.g.
/// `/dev/nvme0n1` or `/dev/sda`) or one of its partitions
/// (`/dev/nvme0n1p1`, `/dev/sda1`, ...).
fn is_same_disk(mount_source: &str, canonical_disk: &str) -> bool {
    if mount_source == canonical_disk {
        return true;
    }
    let Some(suffix) = mount_source.strip_prefix(canonical_disk) else {
        return false;
    };
    let partition_number = suffix.strip_prefix('p').unwrap_or(suffix);
    !partition_number.is_empty() && partition_number.chars().all(|c| c.is_ascii_digit())
}

/// Every mounted device (from the contents of `/proc/mounts`) that's the
/// same disk as `canonical_disk`, or one of its partitions.
pub fn mounted_partitions_of(canonical_disk: &str, proc_mounts: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in proc_mounts.lines() {
        let Some(device) = line.split_whitespace().next() else {
            continue;
        };
        if is_same_disk(device, canonical_disk) && !found.contains(&device.to_string()) {
            found.push(device.to_string());
        }
    }
    found
}

/// The device backing `/`, from the contents of `/proc/mounts`.
pub fn running_system_device(proc_mounts: &str) -> Option<String> {
    proc_mounts.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let device = fields.next()?;
        let mount_point = fields.next()?;
        (mount_point == "/").then(|| device.to_string())
    })
}

pub fn check_disk_not_mounted(canonical_disk: &str, proc_mounts: &str) -> Result<(), SafetyError> {
    let mounted = mounted_partitions_of(canonical_disk, proc_mounts);
    if mounted.is_empty() {
        Ok(())
    } else {
        Err(SafetyError::DiskIsMounted {
            device: canonical_disk.to_string(),
            mounted,
        })
    }
}

pub fn check_disk_not_running_system(
    canonical_disk: &str,
    canonical_root_device: &str,
) -> Result<(), SafetyError> {
    if is_same_disk(canonical_root_device, canonical_disk) {
        Err(SafetyError::DiskHoldsRunningSystem {
            device: canonical_disk.to_string(),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_must_match_the_plan_device_exactly() {
        assert_eq!(
            check_target("/dev/disk/by-id/nvme-X", "/dev/disk/by-id/nvme-X"),
            Ok(())
        );
        assert!(check_target("/dev/disk/by-id/nvme-X", "/dev/disk/by-id/nvme-Y").is_err());
    }

    #[test]
    fn recognizes_sata_style_partitions() {
        assert!(is_same_disk("/dev/sda1", "/dev/sda"));
        assert!(is_same_disk("/dev/sda", "/dev/sda"));
        assert!(!is_same_disk("/dev/sdb1", "/dev/sda"));
    }

    #[test]
    fn recognizes_nvme_style_partitions() {
        assert!(is_same_disk("/dev/nvme0n1p1", "/dev/nvme0n1"));
        assert!(!is_same_disk("/dev/nvme0n1p1", "/dev/nvme1n1"));
    }

    #[test]
    fn does_not_treat_a_different_disk_sharing_a_prefix_as_the_same() {
        // /dev/sda1 must not match canonical disk /dev/sd (a nonsense
        // disk, but the point is the suffix-after-prefix must be purely
        // numeric partition digits, not an arbitrary string).
        assert!(!is_same_disk("/dev/sdaa1", "/dev/sda"));
    }

    #[test]
    fn finds_mounted_partitions_of_the_target_disk() {
        let proc_mounts = "\
/dev/sda1 /boot vfat rw 0 0
/dev/sda2 / ext4 rw 0 0
/dev/sdb1 /mnt/other ext4 rw 0 0
tmpfs /run tmpfs rw 0 0
";
        let mounted = mounted_partitions_of("/dev/sda", proc_mounts);
        assert_eq!(
            mounted,
            vec!["/dev/sda1".to_string(), "/dev/sda2".to_string()]
        );
        assert!(check_disk_not_mounted("/dev/sda", proc_mounts).is_err());
        assert_eq!(check_disk_not_mounted("/dev/sdc", proc_mounts), Ok(()));
    }

    #[test]
    fn finds_the_running_systems_device() {
        let proc_mounts = "\
/dev/nvme0n1p1 /boot vfat rw 0 0
/dev/nvme0n1p2 / btrfs rw 0 0
";
        assert_eq!(
            running_system_device(proc_mounts),
            Some("/dev/nvme0n1p2".to_string())
        );
    }

    #[test]
    fn refuses_a_disk_that_holds_the_running_system() {
        assert!(check_disk_not_running_system("/dev/nvme0n1", "/dev/nvme0n1p2").is_err());
        assert_eq!(
            check_disk_not_running_system("/dev/nvme1n1", "/dev/nvme0n1p2"),
            Ok(())
        );
    }
}
