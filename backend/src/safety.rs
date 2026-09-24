// SPDX-License-Identifier: GPL-3.0-or-later

//! The non-negotiable checks from CLAUDE.md's safety rules: a real run
//! needs `--target` to match the plan's disk exactly, and refuses any
//! disk that's mounted or holds the running system. Also refused: the
//! medium the live system booted from, even once archiso has copied it
//! to RAM and unmounted it, and disks below `installer.toml`'s
//! `min_disk_gib` (SPEC.md "Disk safety"). Pure functions here so
//! they're testable without real disks; `live.rs` and `main.rs` wire
//! them to `/proc`, `/sys` and `std::fs::canonicalize`.

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
    #[error("{device} is the medium the live system booted from")]
    DiskIsBootMedium { device: String },
    #[error("{device} is {size_gib} GiB; installing needs at least {min_gib} GiB")]
    DiskTooSmall {
        device: String,
        size_gib: u64,
        min_gib: u64,
    },
}

const GIB: u64 = 1024 * 1024 * 1024;

pub fn check_disk_size(device: &str, size_bytes: u64, min_gib: u64) -> Result<(), SafetyError> {
    if size_bytes >= min_gib.saturating_mul(GIB) {
        Ok(())
    } else {
        Err(SafetyError::DiskTooSmall {
            device: device.to_string(),
            size_gib: size_bytes / GIB,
            min_gib,
        })
    }
}

/// Where the kernel command line says archiso's boot medium is, as a
/// `/dev/...` path still to be resolved: `archisosearchuuid=` (what
/// current releng boot entries use), `archisodevice=` (a path or a
/// `UUID=`/`LABEL=`/`PARTUUID=` spec) or `archisolabel=`.
pub fn boot_medium_path(cmdline: &str) -> Option<String> {
    for param in cmdline.split_whitespace() {
        if let Some(uuid) = param.strip_prefix("archisosearchuuid=") {
            return Some(format!("/dev/disk/by-uuid/{uuid}"));
        }
        if let Some(device) = param.strip_prefix("archisodevice=") {
            let path = if let Some(uuid) = device.strip_prefix("UUID=") {
                format!("/dev/disk/by-uuid/{uuid}")
            } else if let Some(label) = device.strip_prefix("LABEL=") {
                format!("/dev/disk/by-label/{label}")
            } else if let Some(partuuid) = device.strip_prefix("PARTUUID=") {
                format!("/dev/disk/by-partuuid/{partuuid}")
            } else {
                device.to_string()
            };
            return Some(path);
        }
        if let Some(label) = param.strip_prefix("archisolabel=") {
            return Some(format!("/dev/disk/by-label/{label}"));
        }
    }
    None
}

/// `boot_medium_disk` is the whole disk the boot medium lives on, already
/// resolved (see `live.rs`); `None` when the system didn't boot from an
/// archiso medium at all.
pub fn check_disk_not_boot_medium(
    canonical_disk: &str,
    boot_medium_disk: Option<&str>,
) -> Result<(), SafetyError> {
    match boot_medium_disk {
        Some(medium) if medium == canonical_disk => Err(SafetyError::DiskIsBootMedium {
            device: canonical_disk.to_string(),
        }),
        _ => Ok(()),
    }
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
    fn finds_the_boot_medium_from_current_releng_entries() {
        let cmdline = "initrd=\\arch\\boot\\x86_64\\initramfs-linux.img archisobasedir=arch \
                       archisosearchuuid=2026-09-24-10-29-24-00 copytoram=y";
        assert_eq!(
            boot_medium_path(cmdline),
            Some("/dev/disk/by-uuid/2026-09-24-10-29-24-00".into())
        );
    }

    #[test]
    fn finds_the_boot_medium_from_older_parameters() {
        assert_eq!(
            boot_medium_path("archisobasedir=arch archisodevice=UUID=abcd-1234"),
            Some("/dev/disk/by-uuid/abcd-1234".into())
        );
        assert_eq!(
            boot_medium_path("archisodevice=/dev/sr0"),
            Some("/dev/sr0".into())
        );
        assert_eq!(
            boot_medium_path("archisolabel=LUMINOS_202609"),
            Some("/dev/disk/by-label/LUMINOS_202609".into())
        );
    }

    #[test]
    fn a_normal_boot_has_no_boot_medium() {
        assert_eq!(boot_medium_path("root=UUID=abcd rw quiet"), None);
    }

    #[test]
    fn refuses_the_boot_medium() {
        assert!(check_disk_not_boot_medium("/dev/sdb", Some("/dev/sdb")).is_err());
        assert_eq!(
            check_disk_not_boot_medium("/dev/vda", Some("/dev/sdb")),
            Ok(())
        );
        assert_eq!(check_disk_not_boot_medium("/dev/vda", None), Ok(()));
    }

    #[test]
    fn refuses_a_disk_below_the_minimum_size() {
        assert_eq!(check_disk_size("/dev/vda", 32 * GIB, 32), Ok(()));
        assert_eq!(
            check_disk_size("/dev/vda", 20 * GIB, 32),
            Err(SafetyError::DiskTooSmall {
                device: "/dev/vda".into(),
                size_gib: 20,
                min_gib: 32,
            })
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
