use anyhow::{anyhow, Result};
use std::path::Path;
use tokio::process::Command;

pub async fn run(args: &[&str]) -> Result<String> {
    run_in(None::<&Path>, args).await
}

pub async fn run_in<P: AsRef<Path>>(cwd: Option<P>, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(p) = cwd {
        cmd.current_dir(p);
    }
    let out = cmd.output().await?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub async fn run_inherit(args: &[&str]) -> Result<()> {
    let status = Command::new("git").args(args).status().await?;
    if !status.success() {
        return Err(anyhow!("git {:?} failed", args));
    }
    Ok(())
}

pub async fn ensure_repo() -> Result<()> {
    run(&["rev-parse", "--is-inside-work-tree"]).await?;
    Ok(())
}

pub async fn current_branch() -> Result<String> {
    run(&["rev-parse", "--abbrev-ref", "HEAD"]).await
}

pub async fn detect_base() -> String {
    detect_base_in(None::<&Path>).await
}

pub async fn detect_base_in<P: AsRef<Path> + Clone>(cwd: Option<P>) -> String {
    if let Ok(h) = run_in(cwd.clone(), &["symbolic-ref", "refs/remotes/origin/HEAD"]).await {
        return h.replace("refs/remotes/origin/", "");
    }
    for b in ["main", "master"] {
        if run_in(
            cwd.clone(),
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/remotes/origin/{b}"),
            ],
        )
        .await
        .is_ok()
        {
            return b.to_string();
        }
    }
    "main".to_string()
}
