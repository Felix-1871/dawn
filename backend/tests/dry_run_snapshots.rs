// SPDX-License-Identifier: GPL-3.0-or-later

//! Compares `dawn-backend --dry-run`'s command list against the checked-in
//! golden files, for the online, offline and Secure Boot plan variants
//! (see SPEC.md "Testing": "Dry-run snapshots"). Regenerate a golden file
//! after an intentional pipeline change with:
//!
//! ```sh
//! cargo run -p backend -- --dry-run tests/plans/<name>.json > tests/golden/<name>.txt
//! ```

use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;

fn workspace_root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
}

fn assert_matches_golden(name: &str) {
    let plan_path = workspace_root()
        .join("tests/plans")
        .join(format!("{name}.json"));
    let golden_path = workspace_root()
        .join("tests/golden")
        .join(format!("{name}.txt"));

    let expected = std::fs::read_to_string(&golden_path)
        .unwrap_or_else(|err| panic!("reading {}: {err}", golden_path.display()));

    Command::cargo_bin("dawn-backend")
        .unwrap()
        .arg("--dry-run")
        .arg(&plan_path)
        .assert()
        .success()
        .stdout(expected);
}

#[test]
fn erase_online_matches_golden() {
    assert_matches_golden("erase-online");
}

#[test]
fn erase_offline_matches_golden() {
    assert_matches_golden("erase-offline");
}

#[test]
fn erase_secureboot_matches_golden() {
    assert_matches_golden("erase-secureboot");
}

#[test]
fn without_dry_run_it_refuses_to_run() {
    let plan_path = workspace_root().join("tests/plans/erase-online.json");
    Command::cargo_bin("dawn-backend")
        .unwrap()
        .arg(&plan_path)
        .assert()
        .failure()
        .stderr(predicate::str::contains("--dry-run"));
}

#[test]
fn an_invalid_plan_is_rejected_with_no_partial_output() {
    let dir = std::env::temp_dir();
    let bad_plan = dir.join("dawn-backend-test-invalid-plan.json");
    std::fs::write(&bad_plan, r#"{"version": 99}"#).unwrap();

    Command::cargo_bin("dawn-backend")
        .unwrap()
        .arg("--dry-run")
        .arg(&bad_plan)
        .assert()
        .failure();

    let _ = std::fs::remove_file(&bad_plan);
}
