// SPDX-License-Identifier: GPL-3.0-or-later

//! The backend's read-only requests: `list_disks`, `probe_firmware` and
//! `check_online` (SPEC.md "Architecture"). None needs root, which is
//! why the GUI's probe backend runs as the live user (DECISIONS.md, M3).

use std::collections::{HashMap, HashSet};

use plan::protocol::DiskInfo;
use serde::Deserialize;

use crate::live::LiveSystem;
use crate::repos;
use crate::runner::{self, Invocation};

const GIB: u64 = 1024 * 1024 * 1024;

/// Every disk an install could go onto (SPEC.md "Disk safety"): the boot
/// medium, the running system's disk and any disk with a mounted
/// partition are left out entirely. Disks too small for
/// `min_disk_gib`, read-only, or with no `/dev/disk/by-id/` name to put
/// in a plan are listed, marked unavailable, so the Disk screen can say
/// why.
pub fn list_disks(min_disk_gib: u64, live: &LiveSystem) -> Result<Vec<DiskInfo>, String> {
    let json = runner::capture_stdout(&Invocation::new(
        "lsblk",
        [
            "--json",
            "--list",
            "--bytes",
            "--output",
            "NAME,PKNAME,PATH,TYPE,SIZE,MODEL,SERIAL,RO,MOUNTPOINTS",
        ],
    ))
    .map_err(|err| err.to_string())?;
    let hidden: Vec<String> = [&live.boot_medium, &live.running_system]
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    disks_from_lsblk(&json, &by_id_links(), &hidden, min_disk_gib)
}

#[derive(Deserialize)]
struct Lsblk {
    blockdevices: Vec<BlockDevice>,
}

#[derive(Deserialize)]
struct BlockDevice {
    name: String,
    pkname: Option<String>,
    path: String,
    #[serde(rename = "type")]
    kind: String,
    size: u64,
    model: Option<String>,
    serial: Option<String>,
    ro: bool,
    #[serde(default)]
    mountpoints: Vec<Option<String>>,
}

/// `lsblk --json --list` output, `/dev/disk/by-id/` names keyed by the
/// device they point at, and whole disks to leave out.
fn disks_from_lsblk(
    json: &str,
    by_id: &HashMap<String, String>,
    hidden: &[String],
    min_disk_gib: u64,
) -> Result<Vec<DiskInfo>, String> {
    let lsblk: Lsblk = serde_json::from_str(json)
        .map_err(|err| format!("could not read lsblk's output: {err}"))?;
    let parents: HashMap<&str, Option<&str>> = lsblk
        .blockdevices
        .iter()
        .map(|d| (d.name.as_str(), d.pkname.as_deref()))
        .collect();

    // A disk counts as mounted if it, or anything on it (a partition, an
    // LVM volume on a partition, ...), is mounted.
    let mut mounted: HashSet<&str> = HashSet::new();
    for device in &lsblk.blockdevices {
        if device.mountpoints.iter().flatten().next().is_none() {
            continue;
        }
        let mut name = device.name.as_str();
        mounted.insert(name);
        while let Some(Some(parent)) = parents.get(name) {
            mounted.insert(parent);
            name = parent;
        }
    }

    let mut disks = Vec::new();
    for device in &lsblk.blockdevices {
        if device.kind != "disk"
            || device.name.starts_with("zram")
            || mounted.contains(device.name.as_str())
            || hidden.contains(&device.path)
        {
            continue;
        }
        let by_id_name = by_id.get(&device.path);
        let unavailable = if by_id_name.is_none() {
            Some("has no /dev/disk/by-id/ name to install by".to_string())
        } else if device.ro {
            Some("is read-only".to_string())
        } else if device.size < min_disk_gib.saturating_mul(GIB) {
            Some(format!(
                "is smaller than the {min_disk_gib} GiB an install needs"
            ))
        } else {
            None
        };
        let clean = |value: &Option<String>| value.as_deref().unwrap_or("").trim().to_string();
        disks.push(DiskInfo {
            device: match by_id_name {
                Some(name) => format!("/dev/disk/by-id/{name}"),
                None => device.path.clone(),
            },
            model: match clean(&device.model) {
                model if model.is_empty() => "Unknown disk".to_string(),
                model => model,
            },
            serial: clean(&device.serial),
            size_bytes: device.size,
            unavailable,
        });
    }
    disks.sort_by(|a, b| a.device.cmp(&b.device));
    Ok(disks)
}

/// `/dev/disk/by-id/` names by the whole-disk device they point at, one
/// per disk. Partitions are left out.
fn by_id_links() -> HashMap<String, String> {
    let mut candidates: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/dev/disk/by-id") else {
        return HashMap::new();
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.contains("-part") {
            continue;
        }
        if let Ok(target) = std::fs::canonicalize(entry.path()) {
            candidates
                .entry(target.to_string_lossy().into_owned())
                .or_default()
                .push(name);
        }
    }
    candidates
        .into_iter()
        .filter_map(|(device, names)| preferred_link(&names).map(|name| (device, name)))
        .collect()
}

/// The name a person would recognise: model and serial (`ata-...`,
/// `nvme-...`, `virtio-...`) over bare identifiers (`wwn-...`,
/// `nvme-eui....`), then the shortest, so `nvme-X` wins over its
/// namespace alias `nvme-X_1`.
fn preferred_link(names: &[String]) -> Option<String> {
    let opaque = |name: &str| {
        ["wwn-", "nvme-eui.", "nvme-nvme."]
            .iter()
            .any(|prefix| name.starts_with(prefix))
    };
    names
        .iter()
        .min_by_key(|name| (opaque(name), name.len(), name.as_str()))
        .cloned()
}

/// UEFI's global variable namespace, where SecureBoot and SetupMode live.
const EFI_GLOBAL_GUID: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";

pub struct Firmware {
    pub uefi: bool,
    pub secure_boot: bool,
    pub setup_mode: bool,
}

pub fn probe_firmware() -> Firmware {
    let flag = |name: &str| {
        std::fs::read(format!(
            "/sys/firmware/efi/efivars/{name}-{EFI_GLOBAL_GUID}"
        ))
        .ok()
        .and_then(|bytes| efivar_flag(&bytes))
        .unwrap_or(false)
    };
    Firmware {
        uefi: std::path::Path::new("/sys/firmware/efi").is_dir(),
        secure_boot: flag("SecureBoot"),
        setup_mode: flag("SetupMode"),
    }
}

/// An efivarfs file is four bytes of attributes, then the value.
fn efivar_flag(bytes: &[u8]) -> Option<bool> {
    bytes.get(4).map(|&value| value == 1)
}

/// Whether every repository in `pacman_conf` has a mirror serving its
/// database.
pub fn check_online(pacman_conf: &str) -> bool {
    let Ok(conf) = std::fs::read_to_string(pacman_conf) else {
        return false;
    };
    let repos = repos::parse_pacman_conf(&conf, &|path| std::fs::read_to_string(path).ok());
    !repos.is_empty()
        && repos.iter().all(|repo| {
            repo.database_urls()
                .iter()
                .any(|url| runner::capture_stdout(&runner::head_request(url)).is_ok())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSBLK: &str = r#"{"blockdevices": [
        {"name":"sda","pkname":null,"path":"/dev/sda","type":"disk","size":8001563222016,"model":"ST8000DM004-2U9188  ","serial":"WSC2R6GY","ro":false,"mountpoints":[]},
        {"name":"sda4","pkname":"sda","path":"/dev/sda4","type":"part","size":8001561821184,"model":null,"serial":null,"ro":false,"mountpoints":["/home"]},
        {"name":"sdb","pkname":null,"path":"/dev/sdb","type":"disk","size":2000398934016,"model":"TOSHIBA HDWD120","serial":"59KXK6AGS","ro":false,"mountpoints":[]},
        {"name":"sdc","pkname":null,"path":"/dev/sdc","type":"disk","size":16106127360,"model":"USB stick","serial":"STICK","ro":false,"mountpoints":[null]},
        {"name":"sr0","pkname":null,"path":"/dev/sr0","type":"rom","size":1073741824,"model":"QEMU DVD-ROM","serial":"QM00003","ro":true,"mountpoints":[]},
        {"name":"vda","pkname":null,"path":"/dev/vda","type":"disk","size":42949672960,"model":null,"serial":"dawn-target","ro":false,"mountpoints":[]},
        {"name":"vdb","pkname":null,"path":"/dev/vdb","type":"disk","size":42949672960,"model":null,"serial":null,"ro":false,"mountpoints":[]},
        {"name":"zram0","pkname":null,"path":"/dev/zram0","type":"disk","size":33174847488,"model":null,"serial":null,"ro":false,"mountpoints":["[SWAP]"]}
    ]}"#;

    fn by_id() -> HashMap<String, String> {
        [
            ("/dev/sda", "ata-ST8000DM004-2U9188_WSC2R6GY"),
            ("/dev/sdb", "ata-TOSHIBA_HDWD120_59KXK6AGS"),
            ("/dev/sdc", "usb-USB_stick_STICK-0:0"),
            ("/dev/vda", "virtio-dawn-target"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    #[test]
    fn lists_installable_disks_by_their_stable_names() {
        let disks = disks_from_lsblk(LSBLK, &by_id(), &["/dev/sdb".into()], 32).unwrap();
        let devices: Vec<_> = disks.iter().map(|d| d.device.as_str()).collect();
        // sda has a mounted partition, sdb is hidden (the boot medium, say),
        // sr0 isn't a disk and zram0 is swap in RAM.
        assert_eq!(
            devices,
            [
                "/dev/disk/by-id/usb-USB_stick_STICK-0:0",
                "/dev/disk/by-id/virtio-dawn-target",
                "/dev/vdb"
            ]
        );
        let target = &disks[1];
        assert_eq!(target.model, "Unknown disk");
        assert_eq!(target.serial, "dawn-target");
        assert_eq!(target.unavailable, None);
    }

    #[test]
    fn marks_small_and_unaddressable_disks_unavailable() {
        let disks = disks_from_lsblk(LSBLK, &by_id(), &[], 32).unwrap();
        let stick = disks.iter().find(|d| d.serial == "STICK").unwrap();
        assert_eq!(
            stick.unavailable.as_deref(),
            Some("is smaller than the 32 GiB an install needs")
        );
        let no_id = disks.iter().find(|d| d.device == "/dev/vdb").unwrap();
        assert_eq!(
            no_id.unavailable.as_deref(),
            Some("has no /dev/disk/by-id/ name to install by")
        );
    }

    #[test]
    fn trims_model_names() {
        let disks = disks_from_lsblk(LSBLK, &by_id(), &[], 32).unwrap();
        assert!(disks.iter().all(|d| d.model != "ST8000DM004-2U9188  "));
    }

    #[test]
    fn prefers_recognisable_short_by_id_names() {
        let names = [
            "nvme-eui.00000000000000000000000000000001".to_string(),
            "nvme-CT2000P310SSD8_24364B170F7E_1".to_string(),
            "nvme-CT2000P310SSD8_24364B170F7E".to_string(),
        ];
        assert_eq!(
            preferred_link(&names).as_deref(),
            Some("nvme-CT2000P310SSD8_24364B170F7E")
        );
        let only_wwn = ["wwn-0x5000c500e5a1b2c3".to_string()];
        assert_eq!(
            preferred_link(&only_wwn).as_deref(),
            Some("wwn-0x5000c500e5a1b2c3")
        );
    }

    #[test]
    fn reads_efi_boolean_variables() {
        assert_eq!(efivar_flag(&[6, 0, 0, 0, 1]), Some(true));
        assert_eq!(efivar_flag(&[6, 0, 0, 0, 0]), Some(false));
        assert_eq!(efivar_flag(&[6, 0, 0]), None);
    }
}
