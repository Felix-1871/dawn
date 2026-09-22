// SPDX-License-Identifier: GPL-3.0-or-later

//! `dawn-backend`: the privileged half of Dawn. Probes disks, validates a
//! plan, runs the install pipeline, streams events over a Unix socket —
//! see SPEC.md "Architecture". `--dry-run` prints the command list
//! without touching anything; a real run needs `--target` and passes the
//! safety checks in [`safety`] before anything is executed. Per
//! CLAUDE.md, a real run only ever belongs inside a disposable VM or
//! against a loop device — never on a development machine.

mod adapters;
mod pipeline;
mod runner;
mod safety;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use plan::InstallPlan;

use adapters::Adapter;
use adapters::arch::ArchAdapter;
use runner::{Action, Runner};

/// dawn-backend: the privileged half of the Dawn installer.
#[derive(Parser)]
#[command(name = "dawn-backend")]
struct Cli {
    /// Print the full command list for the plan instead of running it.
    #[arg(long)]
    dry_run: bool,

    /// The disk to install onto — must match the plan's disk exactly.
    /// Required for a real run; ignored (and unnecessary) for --dry-run.
    #[arg(long)]
    target: Option<String>,

    /// Path to an InstallPlan JSON file.
    plan: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let raw = match std::fs::read_to_string(&cli.plan) {
        Ok(raw) => raw,
        Err(err) => {
            eprintln!("error: could not read {}: {err}", cli.plan.display());
            return ExitCode::FAILURE;
        }
    };

    let plan: InstallPlan = match serde_json::from_str(&raw) {
        Ok(plan) => plan,
        Err(err) => {
            eprintln!(
                "error: {} is not a valid InstallPlan: {err}",
                cli.plan.display()
            );
            return ExitCode::FAILURE;
        }
    };

    if let Err(errors) = plan::validate::validate(&plan) {
        eprintln!("error: {} failed validation:", cli.plan.display());
        for error in errors {
            eprintln!("  - {error}");
        }
        return ExitCode::FAILURE;
    }

    let adapter = ArchAdapter;
    let steps = pipeline::build(&plan, &adapter);

    if cli.dry_run {
        println!("==> Adapter: {}", adapter.name());
        let mut dry_run = runner::DryRunRunner::new();
        for step in &steps {
            println!("==> Step {}: {}", step.number, step.name);
            for action in &step.actions {
                let _ = dry_run.run(action);
            }
            for line in dry_run.lines.drain(..) {
                println!("{line}");
            }
        }
        return ExitCode::SUCCESS;
    }

    let Some(target) = &cli.target else {
        eprintln!(
            "error: --target <DEVICE> is required for a real run, and must match the\n\
             plan's disk exactly. Pass --dry-run instead to see the command list.\n\
             Real runs only ever belong inside a disposable VM or against a loop\n\
             device — never on a development machine."
        );
        return ExitCode::FAILURE;
    };

    if let Err(message) = check_target_is_safe(&plan, target) {
        eprintln!("error: refusing to run: {message}");
        return ExitCode::FAILURE;
    }

    let mut runner = runner::RealRunner::new();
    for step in &steps {
        println!("==> Step {}: {}", step.number, step.name);
        for action in &step.actions {
            println!("{}", runner::format_action(action));
            if let Err(err) = runner.run(action) {
                eprintln!("error: step {} ({}) failed: {err}", step.number, step.name);
                best_effort_unmount();
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}

/// Runs the safety checks from CLAUDE.md before any real command is
/// spawned: `--target` must name the plan's own disk, and that disk must
/// be neither mounted nor the one the running system booted from.
fn check_target_is_safe(plan: &InstallPlan, target: &str) -> Result<(), String> {
    safety::check_target(&plan.disk.device, target).map_err(|err| err.to_string())?;

    let canonical_target = std::fs::canonicalize(&plan.disk.device)
        .map_err(|err| format!("could not resolve {}: {err}", plan.disk.device))?
        .to_string_lossy()
        .into_owned();

    let proc_mounts = std::fs::read_to_string("/proc/mounts")
        .map_err(|err| format!("could not read /proc/mounts: {err}"))?;

    safety::check_disk_not_mounted(&canonical_target, &proc_mounts)
        .map_err(|err| err.to_string())?;

    if let Some(root_device) = safety::running_system_device(&proc_mounts) {
        // If the root device isn't a resolvable block device path (an
        // overlay or tmpfs root, say), there's no real disk to compare
        // against, so the check simply doesn't apply.
        if let Ok(canonical_root) = std::fs::canonicalize(&root_device) {
            safety::check_disk_not_running_system(
                &canonical_target,
                &canonical_root.to_string_lossy(),
            )
            .map_err(|err| err.to_string())?;
        }
    }

    Ok(())
}

/// Best-effort cleanup after a failed step, so a retry starts from an
/// unmounted target. Errors here are swallowed: the original failure is
/// already being reported, and the target partitions are left formatted
/// either way per SPEC.md's "On failure" — there's no rollback in v1.
fn best_effort_unmount() {
    let _ = runner::RealRunner::new().run(&Action::Run(runner::Invocation::new(
        "umount",
        ["-R", pipeline::TARGET],
    )));
}
