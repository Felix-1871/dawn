// SPDX-License-Identifier: GPL-3.0-or-later

//! What the backend learns about the live system it runs in: where the
//! offline image is, which disk the live system booted from, and which
//! holds the running system (SPEC.md "Disk safety"). archiso's default
//! `copytoram=auto` copies the image into RAM and unmounts the boot
//! medium on most USB boots, so neither the configured image path nor
//! the mount table alone is enough (DECISIONS.md, M1).

use std::path::{Path, PathBuf};

use plan::config::InstallerConfig;

use crate::safety;

pub struct LiveSystem {
    /// The squashfs image an offline install unpacks: archiso's copy in
    /// RAM when it made one, else the configured path on the medium.
    pub squashfs_image: String,
    /// The whole disk the live system booted from, e.g. `/dev/sdb`, if it
    /// booted from an archiso medium at all.
    pub boot_medium: Option<String>,
    /// The whole disk holding `/`, if `/` is on a disk at all (a live
    /// system's root is an overlay).
    pub running_system: Option<String>,
}

impl LiveSystem {
    pub fn detect(config: &InstallerConfig) -> Self {
        let squashfs_image = image_path(&config.source.squashfs, &|path| Path::new(path).exists());
        let boot_medium = std::fs::read_to_string("/proc/cmdline")
            .ok()
            .and_then(|cmdline| safety::boot_medium_path(&cmdline))
            .and_then(|path| whole_disk(Path::new(&path)));
        let running_system = std::fs::read_to_string("/proc/mounts")
            .ok()
            .and_then(|mounts| safety::running_system_device(&mounts))
            .and_then(|device| whole_disk(Path::new(&device)));
        Self {
            squashfs_image,
            boot_medium,
            running_system,
        }
    }
}

/// The safety checks from CLAUDE.md and SPEC.md, run before any real
/// install: `--target` must name the plan's own disk, and that disk must
/// exist, be big enough, and be none of: mounted, the running system's,
/// or the boot medium (which copytoram may have unmounted, hiding it from
/// the mount check).
pub fn refuse_unsafe_target(
    plan_device: &str,
    target: &str,
    min_disk_gib: u64,
    live: &LiveSystem,
) -> Result<(), String> {
    safety::check_target(plan_device, target).map_err(|err| err.to_string())?;

    let canonical_target = whole_disk(Path::new(plan_device))
        .ok_or_else(|| format!("could not resolve {plan_device} to a disk"))?;
    let resolved = std::fs::canonicalize(plan_device)
        .map_err(|err| format!("could not resolve {plan_device}: {err}"))?;
    if resolved != Path::new(&canonical_target) {
        return Err(format!(
            "{plan_device} is a partition of {canonical_target}, not a whole disk"
        ));
    }

    let proc_mounts = std::fs::read_to_string("/proc/mounts")
        .map_err(|err| format!("could not read /proc/mounts: {err}"))?;
    safety::check_disk_not_mounted(&canonical_target, &proc_mounts)
        .map_err(|err| err.to_string())?;
    if let Some(running_system) = &live.running_system {
        safety::check_disk_not_running_system(&canonical_target, running_system)
            .map_err(|err| err.to_string())?;
    }
    safety::check_disk_not_boot_medium(&canonical_target, live.boot_medium.as_deref())
        .map_err(|err| err.to_string())?;

    let size = disk_size_bytes(&canonical_target)
        .ok_or_else(|| format!("could not read the size of {canonical_target}"))?;
    safety::check_disk_size(&canonical_target, size, min_disk_gib).map_err(|err| err.to_string())
}

/// archiso copies the image to `/run/archiso/copytoram/` under its own
/// file name, then reads it from there.
pub fn image_path(configured: &str, exists: &dyn Fn(&str) -> bool) -> String {
    let file_name = configured.rsplit('/').next().unwrap_or(configured);
    let copied = format!("/run/archiso/copytoram/{file_name}");
    if exists(&copied) {
        copied
    } else {
        configured.to_string()
    }
}

/// The whole disk a block device lives on: `/dev/sdb` for `/dev/sdb1`,
/// or the device itself when it isn't a partition (`/dev/sr0`, a whole
/// USB stick holding an ISO 9660 image). `None` for a path that isn't a
/// block device, such as an overlay root.
pub fn whole_disk(device: &Path) -> Option<String> {
    let canonical = std::fs::canonicalize(device).ok()?;
    let name = canonical.file_name()?.to_str()?.to_string();
    let sys = PathBuf::from("/sys/class/block").join(&name);
    if !sys.exists() {
        return None;
    }
    if sys.join("partition").exists() {
        let parent = std::fs::canonicalize(&sys).ok()?;
        let disk = parent.parent()?.file_name()?.to_str()?;
        Some(format!("/dev/{disk}"))
    } else {
        Some(format!("/dev/{name}"))
    }
}

/// A disk's size from sysfs, which counts 512-byte sectors whatever the
/// disk's own sector size.
pub fn disk_size_bytes(canonical_disk: &str) -> Option<u64> {
    let name = canonical_disk.rsplit('/').next()?;
    let sectors: u64 = std::fs::read_to_string(format!("/sys/class/block/{name}/size"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(sectors * 512)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEDIUM: &str = "/run/archiso/bootmnt/arch/x86_64/airootfs.sfs";

    #[test]
    fn reads_the_image_from_the_medium_by_default() {
        assert_eq!(image_path(MEDIUM, &|_| false), MEDIUM);
    }

    #[test]
    fn prefers_archisos_copy_in_ram() {
        let exists = |path: &str| path == "/run/archiso/copytoram/airootfs.sfs";
        assert_eq!(
            image_path(MEDIUM, &exists),
            "/run/archiso/copytoram/airootfs.sfs"
        );
    }

    #[test]
    fn an_overlay_root_has_no_disk() {
        assert_eq!(whole_disk(Path::new("airootfs")), None);
    }
}
