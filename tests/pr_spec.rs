//! # `ciri pr` — product specification
//!
//! Integration tests for PR generation. We exercise `--dry-run` so no calls
//! to `gh` are required; the mock Ollama server provides the title + body.

mod common;

use common::{ciri_bin, MockOllama, Repo};
use std::process::Command;

fn run_pr(repo: &Repo, mock_url: &str, extra: &[&str]) -> std::process::Output {
    Command::new(ciri_bin())
        .arg("--offline")
        .arg("--host")
        .arg(mock_url)
        .arg("pr")
        .args(extra)
        .current_dir(repo.path())
        .env("CIRI_OFFLINE", "1")
        .output()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refuses_to_run_on_the_base_branch() {
    let repo = Repo::new();
    // still on `main`
    let mock = MockOllama::start("never called").await;
    let out = run_pr(&repo, &mock.url(), &["--dry-run"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("On base branch"), "stderr: {err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refuses_when_there_are_no_commits_ahead_of_base() {
    let repo = Repo::new();
    repo.checkout_new("feat/empty");
    let mock = MockOllama::start("never called").await;
    let out = run_pr(&repo, &mock.url(), &["--dry-run", "--base", "main"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("No commits"), "stderr: {err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dry_run_previews_title_and_body_without_pushing() {
    let repo = Repo::new();
    repo.checkout_new("feat/thing");
    repo.write("new.txt", "added\n");
    repo.stage_all();
    repo.commit("feat: add thing");

    let body = "feat: add thing\n\n## Summary\n- does the thing";
    let mock = MockOllama::start(Box::leak(body.to_string().into_boxed_str())).await;

    let out = run_pr(&repo, &mock.url(), &["--dry-run", "--base", "main"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Title should appear in the rendered preview (we can't match ANSI exactly,
    // but the title text itself survives)
    assert!(stdout.contains("add thing"), "stdout: {stdout}");
    assert!(stdout.contains("Summary"), "stdout: {stdout}");

    // And the remote must not have gained the branch
    let remote_branches = Command::new("git")
        .args([
            "--git-dir",
            repo.remote_dir.path().to_str().unwrap(),
            "branch",
        ])
        .output()
        .unwrap()
        .stdout;
    let s = String::from_utf8_lossy(&remote_branches);
    assert!(
        !s.contains("feat/thing"),
        "branch should not be pushed on --dry-run: {s}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_sent_to_model_includes_branch_base_and_diff_stat() {
    let repo = Repo::new();
    repo.checkout_new("feat/xyz");
    repo.write("f.txt", "content\n");
    repo.stage_all();
    repo.commit("feat: content");

    let mock = MockOllama::start("feat: xyz\n\nbody").await;
    let url = mock.url();
    let _ = run_pr(&repo, &url, &["--dry-run", "--base", "main"]);

    let body = mock.received_body().await;
    // The JSON body contains the user prompt we built in prompts::pr_user_prompt
    assert!(
        body.contains("Branch: feat/xyz -> main"),
        "prompt missing: {body}"
    );
    assert!(
        body.contains("Diff stat:"),
        "prompt missing diff stat: {body}"
    );
    assert!(
        body.contains("feat: content"),
        "prompt missing commit summary: {body}"
    );
}
