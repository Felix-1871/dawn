// SPDX-License-Identifier: GPL-3.0-or-later

//! `dawn-backend`: the privileged half of Dawn. Probes disks, validates a
//! plan, runs the install pipeline, streams events over a Unix socket —
//! see SPEC.md "Architecture". M0 only wires up `--dry-run`: printing the
//! command list for a plan without touching anything. Real execution
//! (M1) is gated behind the safety rules in CLAUDE.md.

mod adapters;
mod pipeline;
mod runner;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use plan::InstallPlan;

use adapters::Adapter;
use adapters::arch::ArchAdapter;

/// dawn-backend: the privileged half of the Dawn installer.
#[derive(Parser)]
#[command(name = "dawn-backend")]
struct Cli {
    /// Print the full command list for the plan instead of running it.
    /// M0 requires this flag; real execution lands in M1.
    #[arg(long)]
    dry_run: bool,

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

    if !cli.dry_run {
        eprintln!(
            "error: refusing to run for real: execution isn't implemented until M1.\n\
             Pass --dry-run to see the command list this plan would produce."
        );
        return ExitCode::FAILURE;
    }

    let adapter = ArchAdapter;
    println!("==> Adapter: {}", adapter.name());

    let steps = pipeline::build(&plan, &adapter);
    let mut dry_run = runner::DryRunRunner::new();

    for step in &steps {
        println!("==> Step {}: {}", step.number, step.name);
        for action in &step.actions {
            dry_run.record(action);
        }
        for line in dry_run.lines.drain(..) {
            println!("{line}");
        }
    }

    ExitCode::SUCCESS
}
