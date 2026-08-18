#![allow(unused_crate_dependencies)]

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn fixture_profile_and_offline_viewer_are_usable_from_the_cli() {
    let temp = tempdir().unwrap();
    let capture = temp.path().join("tail.slice");
    let report = temp.path().join("tail.html");

    Command::cargo_bin("slice")
        .unwrap()
        .args(["fixture-profile", "--output", capture.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote"));

    Command::cargo_bin("slice")
        .unwrap()
        .args([
            "view",
            capture.to_str().unwrap(),
            "--output",
            report.to_str().unwrap(),
            "--percentile",
            "99:100",
        ])
        .assert()
        .success();

    let html = std::fs::read_to_string(report).unwrap();
    assert!(html.contains("SliceFixture::slow_tail_b()"));
    assert!(!html.contains("https://"));
    assert!(!html.contains("fetch("));
}

#[test]
fn bimodal_fixture_shows_slow_path_and_thread_timeline_in_the_viewer() {
    let temp = tempdir().unwrap();
    let capture = temp.path().join("bimodal.slice");
    let report = temp.path().join("bimodal.html");

    Command::cargo_bin("slice")
        .unwrap()
        .args([
            "fixture-profile",
            "--scenario",
            "bimodal",
            "--output",
            capture.to_str().unwrap(),
        ])
        .assert()
        .success();
    Command::cargo_bin("slice")
        .unwrap()
        .args([
            "view",
            capture.to_str().unwrap(),
            "--output",
            report.to_str().unwrap(),
            "--percentile",
            "95:100",
            "--metric",
            "off-cpu",
        ])
        .assert()
        .success();

    let html = std::fs::read_to_string(report).unwrap();
    assert!(html.contains("BimodalFixture::slow_path()"));
    assert!(html.contains("id=\"timeline\""));
    assert!(html.contains("slice-worker-1"));
    assert!(html.contains("Off-CPU time"));
}

#[test]
fn offline_fixture_matrix_covers_wait_metrics_and_discovery() {
    let temp = tempdir().unwrap();
    let capture = temp.path().join("off-cpu.slice");
    let report = temp.path().join("off-cpu.html");

    Command::cargo_bin("slice")
        .unwrap()
        .args([
            "fixture-profile",
            "--scenario",
            "off-cpu",
            "--output",
            capture.to_str().unwrap(),
        ])
        .assert()
        .success();

    Command::cargo_bin("slice")
        .unwrap()
        .args([
            "validate",
            capture.to_str().unwrap(),
            "--require-complete",
            "--require-samples",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("16 invocations"));

    Command::cargo_bin("slice")
        .unwrap()
        .args(["discover", capture.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("sleep_for"));

    Command::cargo_bin("slice")
        .unwrap()
        .args([
            "view",
            capture.to_str().unwrap(),
            "--output",
            report.to_str().unwrap(),
            "--threads",
            "7201",
            "--time",
            "0ms:100ms",
            "--percentile",
            "0:100",
            "--metric",
            "off-cpu",
        ])
        .assert()
        .success();

    let html = std::fs::read_to_string(report).unwrap();
    assert!(html.contains("SliceFixture::sleep_work(unsigned int)"));
    assert!(html.contains("std::this_thread::sleep_for(...)"));
    assert!(html.contains("Off-CPU time"));
    assert!(html.contains("wait-worker-1"));
}

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
fn doctor_explains_privileged_capture_requirements() {
    Command::cargo_bin("slice")
        .unwrap()
        .args(["doctor"])
        .assert()
        .success()
        .stdout(predicate::str::contains("CAP_BPF"))
        .stdout(predicate::str::contains("CAP_PERFMON"))
        .stdout(predicate::str::contains("Max locked memory"))
        .stdout(predicate::str::contains("sudo"));
}

#[test]
fn validate_accepts_a_complete_fixture_profile() {
    let temp = tempdir().unwrap();
    let capture = temp.path().join("valid.slice");

    Command::cargo_bin("slice")
        .unwrap()
        .args(["fixture-profile", "--output", capture.to_str().unwrap()])
        .assert()
        .success();

    Command::cargo_bin("slice")
        .unwrap()
        .args([
            "validate",
            capture.to_str().unwrap(),
            "--require-complete",
            "--require-samples",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("valid profile"));
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
