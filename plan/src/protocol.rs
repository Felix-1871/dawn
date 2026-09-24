// SPDX-License-Identifier: GPL-3.0-or-later

//! The wire protocol between the GUI and `dawn-backend --serve` (SPEC.md
//! "Architecture"): newline-delimited JSON over a Unix socket, one
//! [`Request`] per line from the GUI and one [`Event`] per line back.
//!
//! An `install` request carries the user's password inside the plan, so
//! a request is never logged. `Debug` on a plan still redacts it (see
//! [`crate::Redacted`]).

use serde::{Deserialize, Serialize};

use crate::install_plan::InstallPlan;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    ListDisks,
    /// UEFI, Secure Boot and Setup Mode state.
    ProbeFirmware,
    /// Whether every package repository an online install needs is
    /// reachable.
    CheckOnline,
    Validate {
        plan: InstallPlan,
    },
    /// Only a backend started with `--target` accepts this, and only for
    /// a plan whose disk is that target.
    Install {
        plan: InstallPlan,
    },
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Answers `list_disks`.
    Disks {
        disks: Vec<DiskInfo>,
    },
    /// Answers `probe_firmware`.
    Firmware {
        uefi: bool,
        secure_boot: bool,
        setup_mode: bool,
    },
    /// Answers `check_online`.
    Online {
        online: bool,
    },
    /// Answers `validate`; empty when the plan is valid.
    Validation {
        errors: Vec<String>,
    },
    /// The install moved on: `percent` is the whole install's, 0 to 100.
    Progress {
        step: u32,
        name: String,
        percent: f32,
    },
    Log {
        line: String,
    },
    /// The install stopped (`step` set), or a request couldn't be served
    /// (`step` empty). After an install failure the target is unmounted
    /// and nothing more runs until the next `install`.
    Error {
        step: Option<u32>,
        message: String,
        /// The install's last log lines (SPEC.md: the last 50).
        #[serde(default)]
        log_tail: Vec<String>,
        /// The install failed reaching the package repositories, so the
        /// GUI can offer switching to an offline install (SPEC.md "On
        /// failure").
        #[serde(default)]
        offline_fallback: bool,
    },
    /// The install finished.
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskInfo {
    /// The `/dev/disk/by-id/` path a plan names this disk by.
    pub device: String,
    pub model: String,
    pub serial: String,
    pub size_bytes: u64,
    /// Why this disk can't be installed onto, if it can't; the GUI shows
    /// it greyed out with this reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_tagged_by_name() {
        let json = serde_json::to_string(&Request::ListDisks).unwrap();
        assert_eq!(json, r#"{"request":"list_disks"}"#);
        let parsed: Request = serde_json::from_str(r#"{"request":"cancel"}"#).unwrap();
        assert!(matches!(parsed, Request::Cancel));
    }

    #[test]
    fn events_round_trip() {
        let events = [
            Event::Progress {
                step: 5,
                name: "Install the base system".into(),
                percent: 42.5,
            },
            Event::Log {
                line: "installing glibc...".into(),
            },
            Event::Error {
                step: Some(5),
                message: "pacstrap exited with status 1".into(),
                log_tail: vec!["error: failed to retrieve some files".into()],
                offline_fallback: true,
            },
            Event::Done,
        ];
        for event in events {
            let line = serde_json::to_string(&event).unwrap();
            assert!(!line.contains('\n'));
            let parsed: Event = serde_json::from_str(&line).unwrap();
            assert_eq!(parsed, event);
        }
    }

    #[test]
    fn an_error_without_optional_fields_parses() {
        let parsed: Event =
            serde_json::from_str(r#"{"event":"error","step":null,"message":"no"}"#).unwrap();
        assert_eq!(
            parsed,
            Event::Error {
                step: None,
                message: "no".into(),
                log_tail: vec![],
                offline_fallback: false,
            }
        );
    }

    #[test]
    fn debug_output_of_an_install_request_hides_the_password() {
        let plan: InstallPlan =
            serde_json::from_str(include_str!("../../tests/plans/erase-online.json")).unwrap();
        let request = Request::Install { plan };
        assert!(!format!("{request:?}").contains("correct-horse-battery-staple"));
    }
}
