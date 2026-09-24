// SPDX-License-Identifier: GPL-3.0-or-later

//! SPEC.md's mock-backend mode: canned answers in place of
//! `dawn-backend`, so the screens can be built and clicked through on any
//! laptop without root, and the UI smoke test runs anywhere. Installs
//! report a quick fake progression from another thread, the way the real
//! backend's events arrive.

use std::sync::Mutex;
use std::time::Duration;

use plan::protocol::{DiskInfo, Event};
use plan::{InstallPlan, Source};

use crate::backend::{Backend, EventSink, Firmware};
use crate::wifi::{ConnectError, Network, Wifi};

pub const DISK: &str = "/dev/disk/by-id/nvme-EXAMPLE_SERIAL";
const GIB: u64 = 1024 * 1024 * 1024;

pub struct MockBackend {
    online: bool,
    setup_mode: bool,
    fail_online_installs: bool,
    hold_installs: bool,
    installs: Mutex<Vec<InstallPlan>>,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBackend {
    pub fn new() -> Self {
        Self {
            online: true,
            setup_mode: false,
            fail_online_installs: false,
            hold_installs: false,
            installs: Mutex::new(Vec::new()),
        }
    }

    /// Installs start and then never finish, so the Installing screen
    /// stays up.
    pub fn holding_installs(mut self) -> Self {
        self.hold_installs = true;
        self
    }

    /// The firmware reports Setup Mode, so the Disk screen offers to set
    /// up Secure Boot.
    pub fn in_setup_mode(mut self) -> Self {
        self.setup_mode = true;
        self
    }

    /// `check_online` reports no network, so the Network screen shows.
    pub fn offline(mut self) -> Self {
        self.online = false;
        self
    }

    /// Online installs fail in step 5 the way an unreachable mirror
    /// makes pacstrap fail, offering the offline install instead.
    pub fn failing_online_installs(mut self) -> Self {
        self.fail_online_installs = true;
        self
    }

    /// Every plan an install was started with, oldest first.
    pub fn installs(&self) -> Vec<InstallPlan> {
        self.installs
            .lock()
            .map(|installs| installs.clone())
            .unwrap_or_default()
    }
}

impl Backend for MockBackend {
    fn list_disks(&self) -> Result<Vec<DiskInfo>, String> {
        Ok(vec![
            DiskInfo {
                device: DISK.to_string(),
                model: "EXAMPLE SSD 512GB".to_string(),
                serial: "EXAMPLE_SERIAL".to_string(),
                size_bytes: 512 * GIB,
                unavailable: None,
            },
            DiskInfo {
                device: "/dev/disk/by-id/usb-EXAMPLE_STICK-0:0".to_string(),
                model: "EXAMPLE STICK".to_string(),
                serial: "STICK".to_string(),
                size_bytes: 16 * GIB,
                unavailable: Some("is smaller than the 32 GiB an install needs".to_string()),
            },
        ])
    }

    fn probe_firmware(&self) -> Result<Firmware, String> {
        Ok(Firmware {
            uefi: true,
            secure_boot: false,
            setup_mode: self.setup_mode,
        })
    }

    fn check_online(&self) -> Result<bool, String> {
        Ok(self.online)
    }

    fn validate(&self, plan: &InstallPlan) -> Result<Vec<String>, String> {
        Ok(match plan::validate::validate(plan) {
            Ok(()) => Vec::new(),
            Err(errors) => errors.iter().map(ToString::to_string).collect(),
        })
    }

    fn install(&self, plan: &InstallPlan, on_event: EventSink) -> Result<(), String> {
        if let Ok(mut installs) = self.installs.lock() {
            installs.push(plan.clone());
        }
        let fail = self.fail_online_installs && plan.source == Source::Pacstrap;
        let hold = self.hold_installs;
        std::thread::spawn(move || {
            if hold {
                on_event(Event::Progress {
                    step: 1,
                    name: "Validate plan and re-probe the disk".to_string(),
                    percent: 0.0,
                });
                loop {
                    std::thread::park();
                }
            }
            let steps: &[(u32, &str)] = &[
                (1, "Validate plan and re-probe the disk"),
                (5, "Install the base system"),
                (9, "Build UKIs"),
                (12, "Enable services, copy the log, unmount"),
            ];
            for (n, &(step, name)) in steps.iter().enumerate() {
                on_event(Event::Progress {
                    step,
                    name: name.to_string(),
                    percent: n as f32 / steps.len() as f32 * 100.0,
                });
                on_event(Event::Log {
                    line: format!("==> Step {step}: {name}"),
                });
                std::thread::sleep(Duration::from_millis(20));
                if fail && step == 5 {
                    let log_tail = vec![
                        "==> Step 5: Install the base system".to_string(),
                        "error: failed retrieving file 'glibc-2.44-1-x86_64.pkg.tar.zst' from mirror.example : Connection timed out".to_string(),
                        "error: failed to commit transaction (failed to retrieve some files)".to_string(),
                    ];
                    on_event(Event::Error {
                        step: Some(5),
                        message: "Install the base system: pacstrap exited with status 1"
                            .to_string(),
                        log_tail,
                        offline_fallback: true,
                    });
                    return;
                }
            }
            on_event(Event::Progress {
                step: 12,
                name: "Enable services, copy the log, unmount".to_string(),
                percent: 100.0,
            });
            on_event(Event::Done);
        });
        Ok(())
    }
}

/// Canned Wi-Fi for mock-backend mode: two networks, and joining always
/// works.
pub struct MockWifi;

impl Wifi for MockWifi {
    fn available(&self) -> bool {
        true
    }

    fn scan(&self) -> Result<Vec<Network>, String> {
        Ok(vec![
            Network {
                ssid: "home-wifi".to_string(),
                strength: 80,
                secured: true,
            },
            Network {
                ssid: "cafe".to_string(),
                strength: 35,
                secured: false,
            },
        ])
    }

    fn connect(&self, ssid: &str, _password: &str) -> Result<String, ConnectError> {
        Ok(ssid.to_string())
    }
}
