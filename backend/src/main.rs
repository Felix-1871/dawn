// SPDX-License-Identifier: GPL-3.0-or-later

//! `dawn-backend`: the privileged half of Dawn. Probes disks, validates a
//! plan, runs the install pipeline, streams events — see SPEC.md
//! "Architecture". Two ways in:
//!
//! - a plan file on the command line, for tests, CI and unattended
//!   installs. `--dry-run` prints the command list without touching
//!   anything; a real run needs `--target` and passes the safety checks
//!   first.
//! - `--serve`, the socket protocol the GUI speaks (see `serve.rs`).
//!
//! Per CLAUDE.md, a real run only ever belongs inside a disposable VM or
//! against a loop device — never on a development machine.

mod adapters;
mod install;
mod live;
mod pipeline;
mod probe;
mod repos;
mod runner;
mod safety;
mod serve;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use plan::InstallPlan;
use plan::config::{self, InstallerConfig};

use adapters::Adapter;
use adapters::arch::ArchAdapter;
use live::LiveSystem;
use runner::{Cancel, DryRunRunner, Log, RealRunner};

/// dawn-backend: the privileged half of the Dawn installer.
#[derive(Parser)]
#[command(name = "dawn-backend")]
struct Cli {
    /// Report what an install would do instead of doing it.
    #[arg(long)]
    dry_run: bool,

    /// The disk to install onto — must match the plan's disk exactly.
    /// Required for a real run and for installing in --serve mode.
    #[arg(long)]
    target: Option<String>,

    /// installer.toml to read instead of /etc/dawn/installer.toml.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Speak the GUI's socket protocol on stdin and stdout instead of
    /// installing a plan file.
    #[arg(long, conflicts_with = "plan")]
    serve: bool,

    /// Path to an InstallPlan JSON file.
    #[arg(required_unless_present = "serve")]
    plan: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let config = match load_config(cli.config.as_deref()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    let live = LiveSystem::detect(&config);

    if cli.serve {
        return serve::serve(serve::Options {
            config,
            live,
            target: cli.target,
            dry_run: cli.dry_run,
        });
    }

    let Some(plan_path) = cli.plan else {
        eprintln!("error: a plan file is required without --serve");
        return ExitCode::FAILURE;
    };
    run_plan_file(
        &plan_path,
        cli.dry_run,
        cli.target.as_deref(),
        &config,
        &live,
    )
}

/// `path` when given; otherwise `/etc/dawn/installer.toml`, falling back
/// to the compiled-in copy of `config/installer.toml` when that doesn't
/// exist (a development machine, say).
fn load_config(path: Option<&Path>) -> Result<InstallerConfig, String> {
    let path = match path {
        Some(path) => path,
        None if Path::new(config::SYSTEM_PATH).exists() => Path::new(config::SYSTEM_PATH),
        None => return InstallerConfig::builtin_default().map_err(|err| err.to_string()),
    };
    let raw = std::fs::read_to_string(path)
        .map_err(|err| format!("could not read {}: {err}", path.display()))?;
    InstallerConfig::from_toml_str(&raw).map_err(|err| format!("{}: {err}", path.display()))
}

fn run_plan_file(
    plan_path: &Path,
    dry_run: bool,
    target: Option<&str>,
    config: &InstallerConfig,
    live: &LiveSystem,
) -> ExitCode {
    let raw = match std::fs::read_to_string(plan_path) {
        Ok(raw) => raw,
        Err(err) => {
            eprintln!("error: could not read {}: {err}", plan_path.display());
            return ExitCode::FAILURE;
        }
    };

    let plan: InstallPlan = match serde_json::from_str(&raw) {
        Ok(plan) => plan,
        Err(err) => {
            eprintln!(
                "error: {} is not a valid InstallPlan: {err}",
                plan_path.display()
            );
            return ExitCode::FAILURE;
        }
    };

    if let Err(errors) = plan::validate::validate(&plan) {
        eprintln!("error: {} failed validation:", plan_path.display());
        for error in errors {
            eprintln!("  - {error}");
        }
        return ExitCode::FAILURE;
    }

    let adapter = ArchAdapter::new(config, live);
    let steps = match pipeline::build(&plan, &adapter) {
        Ok(steps) => steps,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    if dry_run {
        println!("==> Adapter: {}", adapter.name());
        let mut log = Log::new(|line| println!("{line}"));
        return match install::run(
            &plan,
            &steps,
            &mut DryRunRunner,
            &mut log,
            &mut |_, _, _| {},
        ) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::FAILURE,
        };
    }

    let Some(target) = target else {
        eprintln!(
            "error: --target <DEVICE> is required for a real run, and must match the\n\
             plan's disk exactly. Pass --dry-run instead to see the command list.\n\
             Real runs only ever belong inside a disposable VM or against a loop\n\
             device — never on a development machine."
        );
        return ExitCode::FAILURE;
    };

    if let Err(message) = live::refuse_unsafe_target(
        &plan.disk.device,
        target,
        config.defaults.min_disk_gib,
        live,
    ) {
        eprintln!("error: refusing to run: {message}");
        return ExitCode::FAILURE;
    }

    let mut log = match Log::with_file(pipeline::LOG_FILE, |line| println!("{line}")) {
        Ok(log) => log,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    log.redact(plan.user.password.expose());

    let mut runner = RealRunner::new(Cancel::new());
    match install::run(&plan, &steps, &mut runner, &mut log, &mut |_, _, _| {}) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => {
            // install::run already logged which step failed and why.
            install::clean_up(&mut log);
            ExitCode::FAILURE
        }
    }
}
