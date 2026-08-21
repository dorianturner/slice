#![allow(unused_crate_dependencies)]

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn profile_command_requires_an_exact_entry_point_and_exposes_capture_controls() {
    Command::cargo_bin("slice")
        .unwrap()
        .args(["profile", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--pid"))
        .stdout(predicate::str::contains("--module"))
        .stdout(predicate::str::contains("--function"))
        .stdout(predicate::str::contains("--duration").not())
        .stdout(predicate::str::contains("PROGRAM"))
        .stdout(predicate::str::contains("--output"));
}

#[test]
fn offline_commands_expose_off_cpu_verification_controls() {
    Command::cargo_bin("slice")
        .unwrap()
        .args(["validate", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--require-off-cpu"));
    Command::cargo_bin("slice")
        .unwrap()
        .args(["discover", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--metric"))
        .stdout(predicate::str::contains("off-cpu"));
}

#[test]
fn doctor_explains_privileged_capture_requirements() {
    Command::cargo_bin("slice")
        .unwrap()
        .args(["doctor"])
        .assert()
        .stdout(predicate::str::contains("Slice doctor (adapter:"))
        .stdout(predicate::str::contains("capture authority"))
        .stdout(predicate::str::contains("memlock limit"))
        .stdout(predicate::str::contains("process-wide uprobe-multi links"))
        .stdout(predicate::str::contains("Doctor summary:"));
}

#[test]
fn validate_rejects_an_invalid_profile_envelope() {
    let temp = tempdir().unwrap();
    let invalid = temp.path().join("invalid.slice");
    std::fs::write(&invalid, b"not a Slice profile").unwrap();

    Command::cargo_bin("slice")
        .unwrap()
        .args(["validate", invalid.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a Slice v1 profile"));
}
