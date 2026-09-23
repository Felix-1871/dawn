// SPDX-License-Identifier: GPL-3.0-or-later

//! A real run must pass the safety checks from CLAUDE.md before anything
//! is executed: `--target` is required, must match the plan's disk
//! exactly, and the disk must resolve to something real. None of these
//! tests touch an actual disk — `tests/plans/erase-online.json` names a
//! `/dev/disk/by-id/...` path that doesn't exist on the test machine,
//! which is exactly the point: resolving it should fail cleanly rather
//! than falling through to running anything.

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

fn plan_path() -> std::path::PathBuf {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/..")).join("tests/plans/erase-online.json")
}

#[test]
fn a_real_run_without_target_is_refused() {
    Command::cargo_bin("dawn-backend")
        .unwrap()
        .arg(plan_path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("--target"));
}

#[test]
fn a_real_run_with_a_mismatched_target_is_refused() {
    Command::cargo_bin("dawn-backend")
        .unwrap()
        .arg("--target")
        .arg("/dev/disk/by-id/nvme-SOME-OTHER-DISK")
        .arg(plan_path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not match"));
}

#[test]
fn a_real_run_against_a_disk_that_does_not_exist_is_refused() {
    // The target matches the plan exactly, but /dev/disk/by-id/nvme-EXAMPLE_SERIAL
    // isn't a real device on the test machine, so this must fail at the
    // "resolve the disk" step rather than proceeding to run anything.
    Command::cargo_bin("dawn-backend")
        .unwrap()
        .arg("--target")
        .arg("/dev/disk/by-id/nvme-EXAMPLE_SERIAL")
        .arg(plan_path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("could not resolve"));
}
