use anyhow::{anyhow, Result};
use clap::Args as ClapArgs;
use tokio::io::{stdout, AsyncWriteExt};

use crate::git;
use crate::ollama::{chat_stream, ChatMessage};
use crate::ui;

const DEFAULT_MODEL: &str = "gemma4:e2b";
const MAX_DIFF: usize = 12_000;

#[derive(ClapArgs)]
pub struct Args {
    /// Skip actually committing — just print the generated message.
    #[arg(long)]
    dry_run: bool,

    /// Don't auto-stage; only use what's already staged.
    #[arg(long)]
    no_add: bool,
}

pub async fn run(args: Args, host: &str, model: Option<&str>) -> Result<()> {
    let model = model.unwrap_or(DEFAULT_MODEL);

    git::ensure_repo().await?;

    if !args.no_add {
        git::run(&["add", "-A"]).await?;
    }

    let diff = git::run(&["diff", "--staged"]).await?;
    if diff.is_empty() {
        return Err(anyhow!("No staged changes."));
    }
    let status = git::run(&["diff", "--staged", "--name-status"]).await?;

    let truncated_diff = if diff.len() > MAX_DIFF {
        format!("{}\n...[truncated]", &diff[..MAX_DIFF])
    } else {
        diff
    };

    let system = "You write Conventional Commit messages.\n\
Rules:\n\
- Format: <type>(<optional scope>): <subject>\n\
- Types: feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert\n\
- Subject: imperative mood, lowercase, no trailing period, <= 72 chars\n\
- Optionally add a blank line then a short body explaining the \"why\" (wrap at 72)\n\
- Output ONLY the commit message. No markdown, no code fences, no commentary.";

    let user = format!(
        "Files changed:\n{}\n\nDiff:\n{}\n\nWrite the commit message.",
        status, truncated_diff
    );

    let messages = vec![
        ChatMessage { role: "system", content: system },
        ChatMessage { role: "user", content: &user },
    ];

    let sp = ui::spinner(format!("Generating commit message with {model}"));
    let mut out = stdout();
    let sp_for_clear = sp.clone();
    let content = chat_stream(host, model, &messages, 0.2, &mut out, move || {
        sp_for_clear.finish_and_clear();
    })
    .await?;
    sp.finish_and_clear();
    out.write_all(b"\n").await?;
    out.flush().await?;

    let message = ui::strip_fences(&content);
    if message.is_empty() {
        return Err(anyhow!("Model returned empty message."));
    }

    if args.dry_run {
        return Ok(());
    }

    git::run_inherit(&["commit", "-m", &message]).await?;
    Ok(())
}
