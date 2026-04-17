use anyhow::{anyhow, Result};
use clap::Args as ClapArgs;
use tokio::io::{stdout, AsyncWriteExt};

use crate::git;
use crate::llm::{select, Message, Mode};
use crate::prompts::{commit_user_prompt, strip_fences, truncate_diff, COMMIT_SYSTEM};
use crate::ui;

/// Default Ollama model when offline.
const OFFLINE_MODEL: &str = "gemma4:e2b";
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

pub async fn run(args: Args, host: &str, model: Option<&str>, mode: Mode) -> Result<()> {
    git::ensure_repo().await?;

    if !args.no_add {
        git::run(&["add", "-A"]).await?;
    }

    let diff = git::run(&["diff", "--staged"]).await?;
    if diff.is_empty() {
        return Err(anyhow!("No staged changes."));
    }
    let status = git::run(&["diff", "--staged", "--name-status"]).await?;

    let (truncated_diff, _) = truncate_diff(&diff, MAX_DIFF);
    let user = commit_user_prompt(&status, &truncated_diff);

    let messages = vec![
        Message {
            role: "system",
            content: COMMIT_SYSTEM,
        },
        Message {
            role: "user",
            content: &user,
        },
    ];

    let provider = select(mode, host, OFFLINE_MODEL, model).await;
    let sp = ui::spinner(format!("Generating commit message via {}", provider.label));

    let mut out = stdout();
    let sp_for_clear = sp.clone();
    let mut on_first = move || sp_for_clear.finish_and_clear();
    let content = provider
        .llm
        .chat(&messages, 0.2, &mut out, &mut on_first)
        .await?;
    sp.finish_and_clear();
    out.write_all(b"\n").await?;
    out.flush().await?;

    let message = strip_fences(&content);
    if message.is_empty() {
        return Err(anyhow!("Model returned empty message."));
    }

    if args.dry_run {
        return Ok(());
    }

    git::run_inherit(&["commit", "-m", &message]).await?;
    ui::render_markdown("\n**✅ committed**\n");
    Ok(())
}
