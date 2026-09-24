// SPDX-License-Identifier: GPL-3.0-or-later

//! How the GUI talks to `dawn-backend` (SPEC.md "Architecture"):
//! newline-delimited JSON over a Unix socket. [`SocketBackend`] starts
//! the backend with one end of a socket pair as its stdin and stdout,
//! which pkexec passes through (it closes every other descriptor).
//!
//! Two backend processes (DECISIONS.md, M3): a probe backend, started as
//! the live user when the GUI starts, answers `list_disks`,
//! `probe_firmware`, `check_online` and `validate`; the install backend,
//! started through pkexec with `--target` once the Summary screen is
//! confirmed, is the only one that installs. The user's password only
//! ever travels in the install request, over that socket.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use plan::InstallPlan;
use plan::protocol::{DiskInfo, Event, Request};

pub struct Firmware {
    pub uefi: bool,
    pub secure_boot: bool,
    pub setup_mode: bool,
}

/// Where install events go. Called from a background thread.
pub type EventSink = Arc<dyn Fn(Event) + Send + Sync>;

pub trait Backend: Send + Sync {
    fn list_disks(&self) -> Result<Vec<DiskInfo>, String>;
    fn probe_firmware(&self) -> Result<Firmware, String>;
    fn check_online(&self) -> Result<bool, String>;
    /// The plan's problems, as the backend sees them; empty when it can
    /// be installed.
    fn validate(&self, plan: &InstallPlan) -> Result<Vec<String>, String>;
    /// Starts installing `plan`; its events go to `on_event` until `Done`
    /// or an `Error`. Calling it again after that installs again from
    /// step 1 (SPEC.md's "Retry from start").
    fn install(&self, plan: &InstallPlan, on_event: EventSink) -> Result<(), String>;
}

/// How to start `dawn-backend`.
#[derive(Clone)]
pub struct BackendCommand {
    /// Absolute path to `dawn-backend`; pkexec needs one, and the polkit
    /// action matches on it.
    pub program: PathBuf,
    /// Run installs as `--dry-run`, as the user, without pkexec: the
    /// development loop SPEC.md's mock-backend mode describes, but
    /// through the real backend.
    pub dry_run: bool,
    /// An `installer.toml` to pass on. Only ever given to backends that
    /// run as the user: the root backend reads the system's own.
    pub config: Option<PathBuf>,
}

impl BackendCommand {
    /// `DAWN_BACKEND` if set, else `dawn-backend` next to this binary (a
    /// cargo build, or /usr/bin on an installed system), else
    /// `/usr/bin/dawn-backend`.
    pub fn locate(dry_run: bool, config: Option<PathBuf>) -> Self {
        let program = std::env::var_os("DAWN_BACKEND")
            .map(PathBuf::from)
            .or_else(|| {
                let sibling = std::env::current_exe().ok()?.parent()?.join("dawn-backend");
                sibling.is_file().then_some(sibling)
            })
            .unwrap_or_else(|| PathBuf::from("/usr/bin/dawn-backend"));
        let program = std::fs::canonicalize(&program).unwrap_or(program);
        Self {
            program,
            dry_run,
            config,
        }
    }
}

struct Probe {
    socket: UnixStream,
    events: BufReader<UnixStream>,
    _child: Child,
}

struct Installer {
    socket: UnixStream,
    /// Set once the backend has gone away (pkexec refused, or it
    /// crashed), so the next install starts a new one.
    gone: Arc<AtomicBool>,
}

pub struct SocketBackend {
    command: BackendCommand,
    probe: Mutex<Option<Probe>>,
    install: Mutex<Option<Installer>>,
    sink: Arc<Mutex<Option<EventSink>>>,
}

impl SocketBackend {
    pub fn new(command: BackendCommand) -> Self {
        Self {
            command,
            probe: Mutex::new(None),
            install: Mutex::new(None),
            sink: Arc::new(Mutex::new(None)),
        }
    }

    fn spawn(&self, extra_args: &[String], as_root: bool) -> Result<(UnixStream, Child), String> {
        let (ours, theirs) = UnixStream::pair().map_err(|err| err.to_string())?;
        let theirs_out = theirs.try_clone().map_err(|err| err.to_string())?;
        let mut command = if as_root {
            let mut pkexec = Command::new("pkexec");
            pkexec.arg(&self.command.program);
            pkexec
        } else {
            Command::new(&self.command.program)
        };
        command.arg("--serve").args(extra_args);
        if !as_root && let Some(config) = &self.command.config {
            command.arg("--config").arg(config);
        }
        let child = command
            .stdin(Stdio::from(OwnedFd::from(theirs)))
            .stdout(Stdio::from(OwnedFd::from(theirs_out)))
            .spawn()
            .map_err(|err| format!("could not start {}: {err}", self.command.program.display()))?;
        Ok((ours, child))
    }

    /// Sends one request to the probe backend (starting it first if
    /// needed) and returns its one reply. A probe backend that stopped
    /// answering is started afresh next time.
    fn ask(&self, request: &Request) -> Result<Event, String> {
        let mut probe = self
            .probe
            .lock()
            .map_err(|_| "the probe backend's lock is poisoned")?;
        if probe.is_none() {
            let (socket, child) = self.spawn(&[], false)?;
            let events = BufReader::new(socket.try_clone().map_err(|err| err.to_string())?);
            *probe = Some(Probe {
                socket,
                events,
                _child: child,
            });
        }
        let Some(running) = probe.as_mut() else {
            return Err("the probe backend didn't start".to_string());
        };
        let reply = exchange(running, request);
        if reply.is_err() {
            *probe = None;
        }
        reply
    }
}

fn exchange(probe: &mut Probe, request: &Request) -> Result<Event, String> {
    send(&mut probe.socket, request)?;
    let mut line = String::new();
    match probe.events.read_line(&mut line) {
        Ok(0) => Err("the probe backend stopped".to_string()),
        Ok(_) => serde_json::from_str(&line)
            .map_err(|err| format!("unreadable reply from the backend: {err}")),
        Err(err) => Err(format!("lost the probe backend: {err}")),
    }
}

fn send(socket: &mut UnixStream, request: &Request) -> Result<(), String> {
    let line = serde_json::to_string(request).map_err(|err| err.to_string())?;
    writeln!(socket, "{line}").map_err(|err| format!("could not reach the backend: {err}"))
}

fn unexpected(event: Event) -> String {
    match event {
        Event::Error { message, .. } => message,
        other => format!("unexpected reply from the backend: {other:?}"),
    }
}

impl Backend for SocketBackend {
    fn list_disks(&self) -> Result<Vec<DiskInfo>, String> {
        match self.ask(&Request::ListDisks)? {
            Event::Disks { disks } => Ok(disks),
            other => Err(unexpected(other)),
        }
    }

    fn probe_firmware(&self) -> Result<Firmware, String> {
        match self.ask(&Request::ProbeFirmware)? {
            Event::Firmware {
                uefi,
                secure_boot,
                setup_mode,
            } => Ok(Firmware {
                uefi,
                secure_boot,
                setup_mode,
            }),
            other => Err(unexpected(other)),
        }
    }

    fn check_online(&self) -> Result<bool, String> {
        match self.ask(&Request::CheckOnline)? {
            Event::Online { online } => Ok(online),
            other => Err(unexpected(other)),
        }
    }

    fn validate(&self, plan: &InstallPlan) -> Result<Vec<String>, String> {
        match self.ask(&Request::Validate { plan: plan.clone() })? {
            Event::Validation { errors } => Ok(errors),
            other => Err(unexpected(other)),
        }
    }

    fn install(&self, plan: &InstallPlan, on_event: EventSink) -> Result<(), String> {
        if let Ok(mut sink) = self.sink.lock() {
            *sink = Some(on_event);
        }
        let mut install = self
            .install
            .lock()
            .map_err(|_| "the install backend's lock is poisoned")?;
        if install
            .as_ref()
            .is_some_and(|running| running.gone.load(Ordering::SeqCst))
        {
            *install = None;
        }
        if install.is_none() {
            let mut args = vec!["--target".to_string(), plan.disk.device.clone()];
            if self.command.dry_run {
                args.push("--dry-run".to_string());
            }
            let (socket, child) = self.spawn(&args, !self.command.dry_run)?;
            let reader = socket.try_clone().map_err(|err| err.to_string())?;
            let gone = Arc::new(AtomicBool::new(false));
            spawn_event_reader(reader, child, Arc::clone(&self.sink), Arc::clone(&gone));
            *install = Some(Installer { socket, gone });
        }
        let Some(running) = install.as_mut() else {
            return Err("the install backend didn't start".to_string());
        };
        send(
            &mut running.socket,
            &Request::Install { plan: plan.clone() },
        )
    }
}

/// Forwards every event the install backend sends to whichever sink is
/// current, and reports the backend going away, which happens when
/// pkexec refuses to start it.
fn spawn_event_reader(
    reader: UnixStream,
    mut child: Child,
    sink: Arc<Mutex<Option<EventSink>>>,
    gone: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let deliver = |event: Event| {
            let current = sink.lock().ok().and_then(|sink| sink.clone());
            if let Some(current) = current {
                current(event);
            }
        };
        for line in BufReader::new(reader).lines() {
            let Ok(line) = line else {
                break;
            };
            if let Ok(event) = serde_json::from_str(&line) {
                deliver(event);
            }
        }
        let status = child
            .wait()
            .map(|status| status.to_string())
            .unwrap_or_else(|err| err.to_string());
        // Before the error goes out, so a Retry from the error screen
        // starts a new backend.
        gone.store(true, Ordering::SeqCst);
        deliver(Event::Error {
            step: None,
            message: format!("The installer's backend stopped ({status})."),
            log_tail: Vec::new(),
            offline_fallback: false,
        });
    });
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// A stand-in for `dawn-backend`: a shell script that ignores its
    /// arguments.
    fn fake_backend(name: &str, script: &str) -> BackendCommand {
        let dir = std::env::temp_dir().join(format!("dawn-fake-backend-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join(name);
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        BackendCommand {
            program,
            dry_run: true,
            config: None,
        }
    }

    fn plan() -> InstallPlan {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/plans/erase-online.json"
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn a_probe_backend_that_stopped_is_started_again() {
        let backend = SocketBackend::new(fake_backend(
            "answers-once",
            r#"read request; echo '{"event":"online","online":true}'"#,
        ));
        assert_eq!(backend.check_online(), Ok(true));
        assert!(
            backend.check_online().is_err(),
            "the first backend only answers once"
        );
        assert_eq!(backend.check_online(), Ok(true));
    }

    #[test]
    fn an_install_backend_that_went_away_is_started_again_for_retry() {
        let backend = SocketBackend::new(fake_backend("exits", "read request; exit 3"));
        let (events, received) = mpsc::channel();
        let sink: EventSink = Arc::new(move |event| {
            let _ = events.send(event);
        });
        for attempt in 1..=2 {
            if let Err(err) = backend.install(&plan(), Arc::clone(&sink)) {
                panic!("attempt {attempt}: {err}");
            }
            match received.recv_timeout(Duration::from_secs(10)) {
                Ok(Event::Error {
                    step: None,
                    message,
                    ..
                }) => assert!(message.contains("exit status: 3"), "{message}"),
                other => panic!("attempt {attempt}: {other:?}"),
            }
        }
    }
}
