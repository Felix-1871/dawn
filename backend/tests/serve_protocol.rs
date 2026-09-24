// SPDX-License-Identifier: GPL-3.0-or-later

//! `dawn-backend --serve`, driven the way the GUI drives it: over one end
//! of a Unix socket pair wired to the backend's stdin and stdout (SPEC.md
//! "Architecture"). Installs here are `--dry-run`, so nothing needs root
//! and nothing touches a disk. `check_online` isn't exercised: it would
//! reach whatever mirrors this machine's pacman.conf names, and tests
//! never talk to public mirrors (CLAUDE.md).

use std::io::{BufRead, BufReader, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Stdio};

use plan::protocol::Event;

const DISK: &str = "/dev/disk/by-id/nvme-EXAMPLE_SERIAL";

fn workspace_root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
}

struct Backend {
    child: Child,
    socket: UnixStream,
    events: BufReader<UnixStream>,
}

impl Backend {
    fn start(extra_args: &[&str]) -> Self {
        let (ours, theirs) = UnixStream::pair().unwrap();
        let theirs_out = theirs.try_clone().unwrap();
        let config = workspace_root().join("config/installer.toml");
        let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("dawn-backend"))
            .arg("--serve")
            .arg("--config")
            .arg(config)
            .args(extra_args)
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .stdout(Stdio::from(OwnedFd::from(theirs_out)))
            .spawn()
            .unwrap();
        let events = BufReader::new(ours.try_clone().unwrap());
        Self {
            child,
            socket: ours,
            events,
        }
    }

    fn send(&mut self, request: &str) {
        writeln!(self.socket, "{request}").unwrap();
    }

    fn next_event(&mut self) -> Event {
        let mut line = String::new();
        self.events.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|err| panic!("{err}: {line:?}"))
    }

    fn install(&mut self, plan_json: &str) {
        self.send(&format!(r#"{{"request":"install","plan":{plan_json}}}"#));
    }

    /// Every event up to and including the install's `done` or `error`.
    fn install_events(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            let event = self.next_event();
            let last = matches!(event, Event::Done | Event::Error { .. });
            events.push(event);
            if last {
                return events;
            }
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        let _ = self.child.wait();
    }
}

fn plan_json() -> String {
    std::fs::read_to_string(workspace_root().join("tests/plans/erase-online.json"))
        .unwrap()
        .replace('\n', " ")
}

#[test]
fn a_dry_run_install_streams_progress_and_log_lines_then_done() {
    let mut backend = Backend::start(&["--dry-run", "--target", DISK]);
    backend.install(&plan_json());
    let events = backend.install_events();

    assert_eq!(events.last(), Some(&Event::Done));
    let percents: Vec<f32> = events
        .iter()
        .filter_map(|event| match event {
            Event::Progress { percent, .. } => Some(*percent),
            _ => None,
        })
        .collect();
    assert_eq!(percents.first(), Some(&0.0));
    assert_eq!(percents.last(), Some(&100.0));
    assert!(percents.windows(2).all(|pair| pair[0] <= pair[1]));

    let lines: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::Log { line } => Some(line.as_str()),
            _ => None,
        })
        .collect();
    assert!(lines.contains(&"==> Step 5: Install the base system"));
    assert!(
        lines
            .iter()
            .all(|line| !line.contains("correct-horse-battery-staple"))
    );
}

#[test]
fn install_can_run_again_after_finishing() {
    // "Retry from start" sends the same install again (DECISIONS.md, M3).
    let mut backend = Backend::start(&["--dry-run", "--target", DISK]);
    backend.install(&plan_json());
    assert_eq!(backend.install_events().last(), Some(&Event::Done));
    backend.install(&plan_json());
    assert_eq!(backend.install_events().last(), Some(&Event::Done));
}

#[test]
fn a_backend_without_target_refuses_to_install() {
    let mut backend = Backend::start(&["--dry-run"]);
    backend.install(&plan_json());
    match backend.next_event() {
        Event::Error { step, message, .. } => {
            assert_eq!(step, None);
            assert!(message.contains("--target"), "{message}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn a_plan_for_another_disk_is_refused() {
    let mut backend = Backend::start(&["--dry-run", "--target", "/dev/disk/by-id/nvme-OTHER"]);
    backend.install(&plan_json());
    match backend.next_event() {
        Event::Error { step, message, .. } => {
            assert_eq!(step, None);
            assert!(message.contains("does not match"), "{message}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn answers_probe_firmware() {
    let mut backend = Backend::start(&[]);
    backend.send(r#"{"request":"probe_firmware"}"#);
    assert!(matches!(backend.next_event(), Event::Firmware { .. }));
}

#[test]
fn validate_reports_plan_errors() {
    let mut backend = Backend::start(&[]);
    let plan = plan_json().replace(r#""hostname": "ada-laptop""#, r#""hostname": "-bad-""#);
    backend.send(&format!(r#"{{"request":"validate","plan":{plan}}}"#));
    match backend.next_event() {
        Event::Validation { errors } => {
            assert!(errors.iter().any(|e| e.contains("hostname")), "{errors:?}");
        }
        other => panic!("expected a validation result, got {other:?}"),
    }
}

#[test]
fn a_garbled_request_gets_an_error_and_the_backend_keeps_serving() {
    let mut backend = Backend::start(&[]);
    backend.send("this is not json");
    assert!(matches!(
        backend.next_event(),
        Event::Error { step: None, .. }
    ));
    // Cancelling with nothing running is ignored, not answered.
    backend.send(r#"{"request":"cancel"}"#);
    backend.send(r#"{"request":"probe_firmware"}"#);
    assert!(matches!(backend.next_event(), Event::Firmware { .. }));
}

#[test]
fn the_backend_exits_when_the_gui_goes_away() {
    let mut backend = Backend::start(&[]);
    backend.socket.shutdown(std::net::Shutdown::Both).unwrap();
    let status = backend.child.wait().unwrap();
    assert!(status.success());
}
