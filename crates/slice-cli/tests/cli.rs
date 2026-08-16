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
fn profile_command_requires_an_exact_entry_point_and_exposes_capture_controls() {
    Command::cargo_bin("slice")
        .unwrap()
        .args(["profile", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--pid"))
        .stdout(predicate::str::contains("--module"))
        .stdout(predicate::str::contains("--function"))
        .stdout(predicate::str::contains("--duration"))
        .stdout(predicate::str::contains("PROGRAM"));
}
