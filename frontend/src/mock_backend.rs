// SPDX-License-Identifier: GPL-3.0-or-later

//! Canned answers for the three probe requests SPEC.md's wire protocol
//! defines (`list_disks`, `probe_firmware`, `check_online`), standing
//! in for the real Unix-socket backend until M3 wires that up. Every
//! screen is built against this same shape of data, so swapping this
//! module for a real socket client later shouldn't touch the UI.

pub struct MockDisk {
    pub device: &'static str,
    pub model: &'static str,
    pub size_gib: i32,
    pub serial: &'static str,
}

pub fn list_disks() -> Vec<MockDisk> {
    vec![MockDisk {
        device: "/dev/disk/by-id/nvme-EXAMPLE_SERIAL",
        model: "EXAMPLE SSD 512GB",
        size_gib: 512,
        serial: "EXAMPLE_SERIAL",
    }]
}

pub struct FirmwareProbe {
    pub is_uefi: bool,
    pub is_setup_mode: bool,
}

pub fn probe_firmware() -> FirmwareProbe {
    FirmwareProbe {
        is_uefi: true,
        is_setup_mode: false,
    }
}

pub fn check_online() -> bool {
    true
}
