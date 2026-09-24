// SPDX-License-Identifier: GPL-3.0-or-later

//! `dawn-backend --serve`: SPEC.md's socket protocol. The GUI starts the
//! backend with one end of a Unix socket pair as its stdin and stdout,
//! then sends one JSON [`Request`] per line and reads one JSON [`Event`]
//! per line back (types in `plan::protocol`). Diagnostics go to stderr;
//! nothing else may be written to stdout.
//!
//! Two backends serve the GUI (DECISIONS.md, M3): one started as the
//! live user without `--target`, for the read-only requests, and one
//! started through pkexec with `--target`, which alone accepts
//! `install`. When the GUI goes away (the socket closes), a running
//! install is cancelled and its target unmounted.

use std::io::{BufRead as _, Write as _};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use plan::InstallPlan;
use plan::config::InstallerConfig;
use plan::protocol::{Event, Request};

use crate::adapters::arch::ArchAdapter;
use crate::install;
use crate::live::{self, LiveSystem};
use crate::pipeline::{self, LOG_FILE, PACMAN_CONF};
use crate::probe;
use crate::runner::{Cancel, DryRunRunner, Log, RealRunner};

pub struct Options {
    pub config: InstallerConfig,
    pub live: LiveSystem,
    /// The disk this backend may install onto; without it, `install` is
    /// refused.
    pub target: Option<String>,
    /// Report installs instead of running them.
    pub dry_run: bool,
}

/// Where events go: stdout, one JSON line each, from whichever thread
/// has one to send.
#[derive(Clone)]
struct Events(Arc<Mutex<std::io::Stdout>>);

impl Events {
    /// False once the GUI can't be written to any more.
    fn send(&self, event: &Event) -> bool {
        let Ok(line) = serde_json::to_string(event) else {
            return false;
        };
        let Ok(mut stdout) = self.0.lock() else {
            return false;
        };
        writeln!(stdout, "{line}")
            .and_then(|()| stdout.flush())
            .is_ok()
    }

    fn request_error(&self, message: impl Into<String>) {
        self.send(&Event::Error {
            step: None,
            message: message.into(),
            log_tail: Vec::new(),
            offline_fallback: false,
        });
    }
}

struct Running {
    thread: JoinHandle<()>,
    cancel: Cancel,
}

pub fn serve(options: Options) -> ExitCode {
    let options = Arc::new(options);
    let events = Events(Arc::new(Mutex::new(std::io::stdout())));
    let mut running: Option<Running> = None;

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        // Never echoed or logged: an install request carries the
        // user's password.
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(err) => {
                events.request_error(format!("not a request this backend understands: {err}"));
                continue;
            }
        };
        if running.as_ref().is_some_and(|r| r.thread.is_finished()) {
            running = None;
        }

        match request {
            Request::ListDisks => {
                match probe::list_disks(options.config.defaults.min_disk_gib, &options.live) {
                    Ok(disks) => {
                        events.send(&Event::Disks { disks });
                    }
                    Err(message) => events.request_error(message),
                }
            }
            Request::ProbeFirmware => {
                let firmware = probe::probe_firmware();
                events.send(&Event::Firmware {
                    uefi: firmware.uefi,
                    secure_boot: firmware.secure_boot,
                    setup_mode: firmware.setup_mode,
                });
            }
            Request::CheckOnline => {
                events.send(&Event::Online {
                    online: probe::check_online(PACMAN_CONF),
                });
            }
            Request::Validate { plan } => {
                events.send(&Event::Validation {
                    errors: validation_errors(&plan, &options),
                });
            }
            Request::Install { plan } => {
                if running.is_some() {
                    events.request_error("an install is already running");
                    continue;
                }
                match start_install(plan, &options, &events) {
                    Ok(started) => running = Some(started),
                    Err(message) => events.request_error(message),
                }
            }
            Request::Cancel => {
                if let Some(running) = &running {
                    running.cancel.request();
                }
            }
        }
    }

    // The GUI went away: stop rather than keep doing root work nobody is
    // watching. The install thread unmounts the target on its way out.
    if let Some(running) = running {
        running.cancel.request();
        let _ = running.thread.join();
    }
    ExitCode::SUCCESS
}

/// The plan's own rules, plus whether its disk is one `list_disks` would
/// offer.
fn validation_errors(plan: &InstallPlan, options: &Options) -> Vec<String> {
    let mut errors: Vec<String> = match plan::validate::validate(plan) {
        Ok(()) => Vec::new(),
        Err(errors) => errors.iter().map(ToString::to_string).collect(),
    };
    match probe::list_disks(options.config.defaults.min_disk_gib, &options.live) {
        Ok(disks) => match disks.iter().find(|disk| disk.device == plan.disk.device) {
            None => errors.push(format!(
                "{} is not a disk this PC can install onto",
                plan.disk.device
            )),
            Some(disk) => {
                if let Some(reason) = &disk.unavailable {
                    errors.push(format!("{} {reason}", plan.disk.device));
                }
            }
        },
        Err(message) => errors.push(message),
    }
    // The GUI only offers Secure Boot in Setup Mode, but the firmware
    // can have changed since, and a plan can come from anywhere.
    if plan.secure_boot.enroll && !probe::probe_firmware().setup_mode {
        errors.push(crate::runner::RunnerError::NotInSetupMode.to_string());
    }
    errors
}

fn start_install(
    plan: InstallPlan,
    options: &Arc<Options>,
    events: &Events,
) -> Result<Running, String> {
    let Some(target) = &options.target else {
        return Err(
            "this backend was started without --target, so it only answers questions; \
                    start one with --target to install"
                .to_string(),
        );
    };
    if let Err(errors) = plan::validate::validate(&plan) {
        let errors: Vec<String> = errors.iter().map(ToString::to_string).collect();
        return Err(format!("the plan failed validation: {}", errors.join("; ")));
    }
    if options.dry_run {
        crate::safety::check_target(&plan.disk.device, target).map_err(|err| err.to_string())?;
    } else {
        live::refuse_unsafe_target(
            &plan.disk.device,
            target,
            options.config.defaults.min_disk_gib,
            &options.live,
        )
        .map_err(|message| format!("refusing to install: {message}"))?;
    }

    let adapter = ArchAdapter::new(&options.config, &options.live);
    let steps = pipeline::build(&plan, &adapter).map_err(|err| err.to_string())?;

    let cancel = Cancel::new();
    let options = Arc::clone(options);
    let thread_events = events.clone();
    let thread_cancel = cancel.clone();
    let thread = std::thread::spawn(move || {
        let events = thread_events;
        let cancel = thread_cancel;
        let line_events = events.clone();
        let line_cancel = cancel.clone();
        let forward = move |line: &str| {
            if !line_events.send(&Event::Log {
                line: line.to_string(),
            }) {
                // The GUI is gone; the main loop sees the socket close
                // too, but there's no reason to keep going until then.
                line_cancel.request();
            }
        };
        let mut log = if options.dry_run {
            Log::new(forward)
        } else {
            match Log::with_file(LOG_FILE, forward) {
                Ok(log) => log,
                Err(err) => {
                    events.request_error(err.to_string());
                    return;
                }
            }
        };
        log.redact(plan.user.password.expose());

        let progress_events = events.clone();
        let mut on_progress = |step: u32, name: &str, percent: f32| {
            progress_events.send(&Event::Progress {
                step,
                name: name.to_string(),
                percent,
            });
        };
        let result = if options.dry_run {
            install::run(&plan, &steps, &mut DryRunRunner, &mut log, &mut on_progress)
        } else {
            install::run(
                &plan,
                &steps,
                &mut RealRunner::new(cancel.clone()),
                &mut log,
                &mut on_progress,
            )
        };

        match result {
            Ok(()) => {
                events.send(&Event::Done);
            }
            Err(failure) => {
                if !options.dry_run {
                    install::clean_up(&mut log);
                }
                events.send(&Event::Error {
                    step: Some(failure.step),
                    message: if failure.cancelled {
                        "the install was cancelled".to_string()
                    } else {
                        format!("{}: {}", failure.name, failure.message)
                    },
                    log_tail: failure.log_tail,
                    offline_fallback: failure.offline_fallback,
                });
            }
        }
    });
    Ok(Running { thread, cancel })
}
