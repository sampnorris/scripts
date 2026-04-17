//! # `ciri commit` — product specification
//!
//! Each test is a single executable sentence describing a user-visible
//! behavior. If a test name would read well in a changelog, it's in the
//! right shape.

mod common;

use common::{ciri_bin, MockOllama, Repo};
use std::process::Command;

/// Spawn `ciri commit` against a test repo and mock Ollama.
fn run_commit(repo: &Repo, mock_url: &str, extra: &[&str]) -> std::process::Output {
    Command::new(ciri_bin())
        .arg("--offline")
        .arg("--host")
        .arg(mock_url)
        .arg("commit")
        .args(extra)
        .current_dir(repo.path())
        .env("CIRI_OFFLINE", "1")
        .output()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stages_all_changes_automatically_and_commits_with_model_message() {
    let repo = Repo::new();
    repo.write("src/a.txt", "hello\n");
    repo.write("src/b.txt", "world\n");
    // Note: deliberately NOT calling stage_all — ciri commit should do it.

    let mock = MockOllama::start("feat: add a and b").await;
    let out = run_commit(&repo, &mock.url(), &[]);
    assert!(
        out.status.success(),
        "ciri commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    assert_eq!(repo.head_message(), "feat: add a and b");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refuses_to_commit_when_there_are_no_changes() {
    let repo = Repo::new();
    let mock = MockOllama::start("should not be used").await;

    let out = run_commit(&repo, &mock.url(), &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("No staged changes"),
        "unexpected stderr: {stderr}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dry_run_does_not_create_a_commit() {
    let repo = Repo::new();
    repo.write("x.txt", "x\n");

    let head_before = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo.path())
        .output()
        .unwrap()
        .stdout;

    let mock = MockOllama::start("chore: nope").await;
    let out = run_commit(&repo, &mock.url(), &["--dry-run"]);
    assert!(out.status.success());

    let head_after = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo.path())
        .output()
        .unwrap()
        .stdout;
    assert_eq!(head_before, head_after, "dry-run must not move HEAD");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_add_flag_only_commits_already_staged_files() {
    let repo = Repo::new();
    repo.write("staged.txt", "yes\n");
    repo.stage_all();
    repo.write("unstaged.txt", "no\n");

    let mock = MockOllama::start("feat: only staged").await;
    let out = run_commit(&repo, &mock.url(), &["--no-add"]);
    assert!(out.status.success());

    // `unstaged.txt` should still be untracked after the commit
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(repo.path())
        .output()
        .unwrap()
        .stdout;
    let s = String::from_utf8_lossy(&status);
    assert!(
        s.contains("unstaged.txt"),
        "unstaged file should remain: {s}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn strips_markdown_fences_that_the_model_accidentally_emits() {
    let repo = Repo::new();
    repo.write("f.txt", "1\n");
    let mock = MockOllama::start("```\nfeat: cleaned\n```").await;
    let out = run_commit(&repo, &mock.url(), &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(repo.head_message(), "feat: cleaned");
}
