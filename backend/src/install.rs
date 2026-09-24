// SPDX-License-Identifier: GPL-3.0-or-later

//! Runs a built pipeline step by step through a [`Runner`], turning what
//! the runner reports into whole-install progress (SPEC.md "Install
//! pipeline": each step has its own progress weight, and step 5
//! dominates), and a failure into what the GUI's error screen needs
//! (SPEC.md "On failure"). The CLI and `--serve` both use it.

use plan::{InstallPlan, Source};

use crate::pipeline::{PipelineStep, TARGET};
use crate::runner::{Action, Cancel, Invocation, Log, RealRunner, Runner, RunnerError, Sink};

/// Why an install stopped.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub step: u32,
    pub name: &'static str,
    pub message: String,
    /// The last lines of the install log, for the error screen.
    pub log_tail: Vec<String>,
    /// It failed reaching the package repositories, so an offline
    /// install is worth offering instead.
    pub offline_fallback: bool,
    pub cancelled: bool,
}

/// How long each step takes relative to the others, for the progress
/// bar. Step 5 (installing the base system) takes most of the install
/// either way, pacstrap or unsquashfs.
fn weight(step: u32) -> f32 {
    match step {
        5 => 60.0,
        9 => 10.0,
        6 | 7 => 4.0,
        2 | 3 | 8 | 10 | 12 => 3.0,
        _ => 2.0,
    }
}

/// Runs every step in order, stopping at the first failure. `on_progress`
/// gets the whole install's percentage (0 to 100) with the step it's in.
pub fn run(
    plan: &InstallPlan,
    steps: &[PipelineStep],
    runner: &mut dyn Runner,
    log: &mut Log,
    on_progress: &mut dyn FnMut(u32, &str, f32),
) -> Result<(), Failure> {
    let total: f32 = steps.iter().map(|step| weight(step.number)).sum();
    let mut done = 0.0;

    for step in steps {
        let step_weight = weight(step.number);
        on_progress(step.number, step.name, percent(done, total));
        log.line(&format!("==> Step {}: {}", step.number, step.name));

        for action in &step.actions {
            let mut sink = StepSink {
                log: &mut *log,
                on_progress: &mut *on_progress,
                step,
                done,
                step_weight,
                total,
            };
            if let Err(error) = runner.run(action, &mut sink) {
                log.line(&format!(
                    "error: step {} ({}) failed: {error}",
                    step.number, step.name
                ));
                let log_tail = log.tail();
                return Err(Failure {
                    step: step.number,
                    name: step.name,
                    offline_fallback: is_network_failure(plan, step.number, &error, &log_tail),
                    cancelled: matches!(error, RunnerError::Cancelled),
                    message: error.to_string(),
                    log_tail,
                });
            }
        }
        done += step_weight;
    }

    if let Some(last) = steps.last() {
        on_progress(last.number, last.name, 100.0);
    }
    Ok(())
}

fn percent(done: f32, total: f32) -> f32 {
    if total > 0.0 {
        (done / total * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    }
}

struct StepSink<'a> {
    log: &'a mut Log,
    on_progress: &'a mut dyn FnMut(u32, &str, f32),
    step: &'a PipelineStep,
    done: f32,
    step_weight: f32,
    total: f32,
}

impl Sink for StepSink<'_> {
    fn line(&mut self, line: &str) {
        self.log.line(line);
    }

    fn progress(&mut self, fraction: f32) {
        let within = self.step_weight * fraction.clamp(0.0, 1.0);
        (self.on_progress)(
            self.step.number,
            self.step.name,
            percent(self.done + within, self.total),
        );
    }
}

/// SPEC.md "On failure": "If pacstrap fails on the network, the error
/// screen also offers switching to the offline install." Step 1 finding
/// no mirror is the same situation, caught earlier.
fn is_network_failure(
    plan: &InstallPlan,
    step: u32,
    error: &RunnerError,
    log_tail: &[String],
) -> bool {
    if plan.source != Source::Pacstrap {
        return false;
    }
    match step {
        1 => matches!(error, RunnerError::ReposUnreachable(_)),
        5 => {
            const SIGNS: &[&str] = &[
                "failed retrieving file",
                "failed to retrieve some files",
                "could not resolve host",
                "failed to connect",
                "connection refused",
                "connection timed out",
                "operation timed out",
            ];
            let text = format!("{}\n{error}", log_tail.join("\n")).to_lowercase();
            SIGNS.iter().any(|sign| text.contains(sign))
        }
        _ => false,
    }
}

/// Best-effort cleanup after a failed real install, so a retry starts
/// from an unmounted target (SPEC.md "On failure": "unmount
/// everything"). Nothing here can make things worse, so errors are
/// logged and otherwise ignored; the target partitions stay formatted,
/// since there's no rollback in v1.
pub fn clean_up(log: &mut Log) {
    let mut runner = RealRunner::new(Cancel::new());
    // A keyring daemon pacstrap started would keep the target busy.
    let _ = runner.run(
        &Action::Run(Invocation::new(
            "gpgconf",
            [
                "--homedir",
                &format!("{TARGET}/etc/pacman.d/gnupg"),
                "--kill",
                "all",
            ],
        )),
        log,
    );
    let unmounted = runner.run(&Action::Run(Invocation::new("umount", ["-R", TARGET])), log);
    if unmounted.is_err() {
        // Whatever still holds the target, detach it now; the kernel
        // finishes the unmount once they let go.
        let _ = runner.run(
            &Action::Run(Invocation::new("umount", ["-R", "-l", TARGET])),
            log,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::runner::DryRunRunner;

    fn plan(source: Source) -> InstallPlan {
        let mut plan: InstallPlan =
            serde_json::from_str(include_str!("../../tests/plans/erase-online.json")).unwrap();
        plan.source = source;
        plan
    }

    fn step(number: u32, actions: Vec<Action>) -> PipelineStep {
        PipelineStep {
            number,
            name: "a step",
            actions,
        }
    }

    fn run_true() -> Action {
        Action::Run(Invocation::new("true", Vec::<String>::new()))
    }

    /// Fails on its `fail_at`th action, reporting `lines` first.
    struct FailingRunner {
        seen: usize,
        fail_at: usize,
        lines: Vec<&'static str>,
        error: fn() -> RunnerError,
    }

    impl Runner for FailingRunner {
        fn run(&mut self, _: &Action, sink: &mut dyn Sink) -> Result<(), RunnerError> {
            self.seen += 1;
            if self.seen == self.fail_at {
                for line in &self.lines {
                    sink.line(line);
                }
                return Err((self.error)());
            }
            sink.progress(0.5);
            Ok(())
        }
    }

    #[test]
    fn progress_climbs_to_100_with_step_5_weighing_most() {
        let steps = vec![
            step(1, vec![run_true()]),
            step(5, vec![run_true()]),
            step(12, vec![run_true()]),
        ];
        let mut seen = Vec::new();
        let mut log = Log::new(|_| {});
        let mut runner = FailingRunner {
            seen: 0,
            fail_at: 0,
            lines: vec![],
            error: || RunnerError::Cancelled,
        };
        run(
            &plan(Source::Pacstrap),
            &steps,
            &mut runner,
            &mut log,
            &mut |step, _, percent| seen.push((step, percent)),
        )
        .unwrap();

        // Total weight 2 + 60 + 3 = 65. Each action reports itself half
        // done, so step 5 passes through (2 + 30) / 65.
        let expected = [
            (1, 0.0),
            (1, 1.0 / 65.0 * 100.0),
            (5, 2.0 / 65.0 * 100.0),
            (5, 32.0 / 65.0 * 100.0),
            (12, 62.0 / 65.0 * 100.0),
            (12, 63.5 / 65.0 * 100.0),
            (12, 100.0),
        ];
        assert_eq!(seen.len(), expected.len());
        for ((step, percent), (want_step, want_percent)) in seen.iter().zip(expected) {
            assert_eq!(*step, want_step);
            assert!(
                (percent - want_percent).abs() < 0.001,
                "{percent} vs {want_percent}"
            );
        }
    }

    #[test]
    fn a_failure_carries_its_step_and_log_tail() {
        let steps = vec![
            step(1, vec![run_true()]),
            step(7, vec![run_true(), run_true()]),
        ];
        let lines = Arc::new(Mutex::new(Vec::new()));
        let logged = lines.clone();
        let mut log = Log::new(move |line| logged.lock().unwrap().push(line.to_string()));
        let mut runner = FailingRunner {
            seen: 0,
            fail_at: 3,
            lines: vec!["locale-gen: no such locale"],
            error: || RunnerError::PatternNotFound {
                path: "/mnt/target/etc/locale.gen".into(),
                pattern: "xx_XX.UTF-8 UTF-8".into(),
            },
        };
        let failure = run(
            &plan(Source::Pacstrap),
            &steps,
            &mut runner,
            &mut log,
            &mut |_, _, _| {},
        )
        .unwrap_err();

        assert_eq!(failure.step, 7);
        assert!(!failure.offline_fallback);
        assert!(!failure.cancelled);
        assert_eq!(failure.log_tail.first().unwrap(), "==> Step 1: a step");
        assert!(
            failure
                .log_tail
                .contains(&"locale-gen: no such locale".to_string())
        );
        assert!(
            failure
                .log_tail
                .last()
                .unwrap()
                .starts_with("error: step 7")
        );
    }

    #[test]
    fn a_pacstrap_download_failure_offers_the_offline_install() {
        let steps = vec![step(5, vec![run_true()])];
        let mut log = Log::new(|_| {});
        let mut runner = FailingRunner {
            seen: 0,
            fail_at: 1,
            lines: vec![
                "error: failed retrieving file 'glibc-2.44-1-x86_64.pkg.tar.zst' from 10.0.2.2:8081 : The requested URL returned error: 404",
                "error: failed to commit transaction (failed to retrieve some files)",
            ],
            error: || RunnerError::NonZeroExit {
                program: "pacstrap".into(),
                status: 1,
                stderr: String::new(),
            },
        };
        let failure = run(
            &plan(Source::Pacstrap),
            &steps,
            &mut runner,
            &mut log,
            &mut |_, _, _| {},
        )
        .unwrap_err();
        assert!(failure.offline_fallback);
    }

    #[test]
    fn unreachable_repositories_in_step_1_offer_the_offline_install() {
        let steps = vec![step(1, vec![run_true()])];
        let mut log = Log::new(|_| {});
        let mut runner = FailingRunner {
            seen: 0,
            fail_at: 1,
            lines: vec![],
            error: || RunnerError::ReposUnreachable(vec!["core".into()]),
        };
        let failure = run(
            &plan(Source::Pacstrap),
            &steps,
            &mut runner,
            &mut log,
            &mut |_, _, _| {},
        )
        .unwrap_err();
        assert!(failure.offline_fallback);
    }

    #[test]
    fn an_offline_install_never_offers_itself_as_the_fallback() {
        let steps = vec![step(5, vec![run_true()])];
        let mut log = Log::new(|_| {});
        let mut runner = FailingRunner {
            seen: 0,
            fail_at: 1,
            lines: vec!["could not resolve host"],
            error: || RunnerError::Cancelled,
        };
        let failure = run(
            &plan(Source::Squashfs),
            &steps,
            &mut runner,
            &mut log,
            &mut |_, _, _| {},
        )
        .unwrap_err();
        assert!(!failure.offline_fallback);
        assert!(failure.cancelled);
    }

    #[test]
    fn dry_run_reports_every_step_and_action() {
        let steps = vec![step(1, vec![run_true()]), step(2, vec![run_true()])];
        let lines = Arc::new(Mutex::new(Vec::new()));
        let logged = lines.clone();
        let mut log = Log::new(move |line| logged.lock().unwrap().push(line.to_string()));
        run(
            &plan(Source::Pacstrap),
            &steps,
            &mut DryRunRunner,
            &mut log,
            &mut |_, _, _| {},
        )
        .unwrap();
        assert_eq!(
            *lines.lock().unwrap(),
            ["==> Step 1: a step", "true", "==> Step 2: a step", "true"]
        );
    }
}
