use anyhow::{anyhow, Result};
use clap::Args as ClapArgs;
use tokio::io::{sink, AsyncWriteExt};
use tokio::process::Command;

use crate::git;
use crate::ollama::{chat_stream, ChatMessage};
use crate::ui;

const DEFAULT_MODEL: &str = "gemma4:e2b";
const MAX_DIFF: usize = 16_000;

#[derive(ClapArgs)]
pub struct Args {
    /// Target base branch (auto-detected if omitted).
    #[arg(long)]
    base: Option<String>,

    /// Open as draft.
    #[arg(long)]
    draft: bool,

    /// Print the generated title/body but don't push or open the PR.
    #[arg(long)]
    dry_run: bool,
}

pub async fn run(args: Args, host: &str, model: Option<&str>) -> Result<()> {
    let model = model.unwrap_or(DEFAULT_MODEL);

    git::ensure_repo().await?;

    let base = match args.base {
        Some(b) => b,
        None => git::detect_base().await,
    };
    let branch = git::current_branch().await?;
    if branch == base {
        return Err(anyhow!("On base branch '{base}'. Checkout a feature branch."));
    }

    let fetch_sp = ui::spinner(format!("Fetching origin/{base}"));
    git::run(&["fetch", "origin", &base]).await?;
    fetch_sp.finish_and_clear();

    let merge_base = git::run(&["merge-base", "HEAD", &format!("origin/{base}")]).await?;
    let commits = git::run(&[
        "log",
        "--pretty=format:- %s",
        &format!("{merge_base}..HEAD"),
    ])
    .await?;
    if commits.is_empty() {
        return Err(anyhow!(
            "No commits on '{branch}' ahead of origin/{base}."
        ));
    }
    let diff_stat = git::run(&["diff", "--stat", &format!("{merge_base}..HEAD")]).await?;
    let diff = git::run(&["diff", &format!("{merge_base}..HEAD")]).await?;
    let truncated_diff = if diff.len() > MAX_DIFF {
        format!("{}\n...[truncated]", &diff[..MAX_DIFF])
    } else {
        diff
    };

    let system = "You write concise, helpful GitHub pull request descriptions.\n\
Output format (markdown):\n  \
<short imperative title on a single line, <= 72 chars, no trailing period>\n  \
<blank line>\n  \
## Summary\n  \
- bullet points of what changed and why\n  \
## Changes\n  \
- key file/area level changes\n  \
## Notes\n  \
- optional: testing, risks, follow-ups (omit section if nothing to say)\n\n\
Rules:\n\
- Title first line only. No \"#\", no quotes, no prefix like \"PR:\".\n\
- Then blank line, then body in markdown.\n\
- No code fences around the whole thing. No commentary.";

    let user = format!(
        "Branch: {branch} -> {base}\n\nCommits:\n{commits}\n\nDiff stat:\n{diff_stat}\n\nDiff:\n{truncated_diff}\n\nWrite the PR title and body."
    );

    let messages = vec![
        ChatMessage { role: "system", content: system },
        ChatMessage { role: "user", content: &user },
    ];

    let sp = ui::spinner(format!("Generating PR description with {model}"));
    // Silently collect tokens; we'll render markdown at the end.
    let mut silent = sink();
    let content = chat_stream(host, model, &messages, 0.2, &mut silent, || {}).await?;
    sp.finish_and_clear();
    silent.flush().await.ok();

    let cleaned = ui::strip_fences(&content);
    let mut lines = cleaned.splitn(2, '\n');
    let title = lines.next().unwrap_or("").trim().trim_start_matches('#').trim().to_string();
    let body = lines.next().unwrap_or("").trim().to_string();

    if title.is_empty() {
        return Err(anyhow!("Model returned empty title."));
    }

    // Pretty preview
    println!();
    let preview = format!("# {title}\n\n{body}\n");
    ui::render_markdown(&preview);
    println!();

    if args.dry_run {
        return Ok(());
    }

    let push_sp = ui::spinner(format!("Pushing {branch} to origin"));
    git::run(&["push", "-u", "origin", &branch]).await?;
    push_sp.finish_and_clear();

    let status = Command::new("gh")
        .arg("pr")
        .arg("create")
        .arg("--base")
        .arg(&base)
        .arg("--head")
        .arg(&branch)
        .arg("--title")
        .arg(&title)
        .arg("--body")
        .arg(&body)
        .args(if args.draft { vec!["--draft"] } else { vec![] })
        .status()
        .await?;
    if !status.success() {
        return Err(anyhow!("gh pr create failed"));
    }
    Ok(())
}
