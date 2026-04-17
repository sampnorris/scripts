//! # `ciri` — CLI contract specification
//!
//! Verifies the top-level command surface stays stable.

mod common;

use common::ciri_bin;
use std::process::Command;

#[test]
fn ciri_help_advertises_the_public_subcommands() {
    let out = Command::new(ciri_bin()).arg("--help").output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("commit"), "help missing `commit`: {s}");
    assert!(s.contains("pr"), "help missing `pr`: {s}");
    assert!(s.contains("--model"), "help missing `--model`: {s}");
    assert!(s.contains("--offline"), "help missing `--offline`: {s}");
    assert!(s.contains("--online"), "help missing `--online`: {s}");
}

#[test]
fn offline_and_online_are_mutually_exclusive() {
    let out = Command::new(ciri_bin())
        .args(["--offline", "--online", "commit"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.to_lowercase().contains("cannot be used"),
        "stderr: {err}"
    );
}

#[test]
fn ciri_version_prints_a_version_string() {
    let out = Command::new(ciri_bin()).arg("--version").output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.trim().starts_with("ciri "));
}
