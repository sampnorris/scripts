use anyhow::{anyhow, Result};
use clap::Args as ClapArgs;
use serde::Deserialize;
use tokio::io::{sink, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::git;
use crate::llm::{select, Message, Mode};
use crate::ui;

const OFFLINE_MODEL: &str = "gemma4:e2b";
const MAX_DIFF: usize = 16_000;

#[derive(ClapArgs)]
pub struct Args {
    /// Target base branch (auto-detected if omitted).
    #[arg(long)]
    base: Option<String>,

    /// Open as draft (only when creating).
    #[arg(long)]
    draft: bool,

    /// Print the generated title/body but don't push, create, or edit anything.
    #[arg(long)]
    dry_run: bool,

    /// Skip the confirmation prompt when updating an existing PR.
    #[arg(long, short = 'y')]
    yes: bool,
}

#[derive(Deserialize)]
struct ExistingPr {
    number: u64,
    url: String,
    title: String,
}

pub async fn run(args: Args, host: &str, model: Option<&str>, mode: Mode) -> Result<()> {
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

    // Detect an existing PR for this branch
    let existing = find_existing_pr(&branch).await?;

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
        Message { role: "system", content: system },
        Message { role: "user", content: &user },
    ];

    let provider = select(mode, host, OFFLINE_MODEL, model).await;
    let sp = ui::spinner(format!("Generating PR description via {}", provider.label));
    let mut silent = sink();
    let mut on_first = || {};
    let content = provider
        .llm
        .chat(&messages, 0.2, &mut silent, &mut on_first)
        .await?;
    sp.finish_and_clear();
    silent.flush().await.ok();

    let cleaned = ui::strip_fences(&content);
    let mut lines = cleaned.splitn(2, '\n');
    let title = lines
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_string();
    let body = lines.next().unwrap_or("").trim().to_string();

    if title.is_empty() {
        return Err(anyhow!("Model returned empty title."));
    }

    // Preview
    println!();
    let header = match &existing {
        Some(pr) => format!("# ✏️  update PR #{} — {title}", pr.number),
        None => format!("# ✨ {title}"),
    };
    let preview = format!("{header}\n\n> _{branch}_ → _{base}_\n\n{body}\n\n---\n");
    ui::render_markdown(&preview);

    if args.dry_run {
        return Ok(());
    }

    // Push before any gh action
    let push_sp = ui::spinner(format!("Pushing {branch} to origin"));
    git::run(&["push", "-u", "origin", &branch]).await?;
    push_sp.finish_and_clear();

    match existing {
        Some(pr) => {
            if !args.yes && !confirm_update(&pr).await? {
                ui::render_markdown("\n_update cancelled — PR left unchanged._\n");
                return Ok(());
            }
            let status = Command::new("gh")
                .args([
                    "pr",
                    "edit",
                    &pr.number.to_string(),
                    "--title",
                    &title,
                    "--body",
                    &body,
                ])
                .status()
                .await?;
            if !status.success() {
                return Err(anyhow!("gh pr edit failed"));
            }
            ui::render_markdown(&format!(
                "\n**🔁 pull request #{} updated**\n\n<{}>\n",
                pr.number, pr.url
            ));
        }
        None => {
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
            ui::render_markdown("\n**🚀 pull request opened!**\n");
        }
    }

    Ok(())
}

async fn find_existing_pr(branch: &str) -> Result<Option<ExistingPr>> {
    let out = Command::new("gh")
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "open",
            "--json",
            "number,url,title",
            "--limit",
            "1",
        ])
        .output()
        .await?;
    if !out.status.success() {
        // gh not configured or repo not on GitHub — treat as "no existing PR"
        return Ok(None);
    }
    let list: Vec<ExistingPr> = serde_json::from_slice(&out.stdout).unwrap_or_default();
    Ok(list.into_iter().next())
}

async fn confirm_update(pr: &ExistingPr) -> Result<bool> {
    ui::render_markdown(&format!(
        "\n**PR #{} already exists:** _{}_\n\n<{}>\n",
        pr.number, pr.title, pr.url
    ));
    eprint!("Update this PR with the new title and body? [y/N] ");
    let mut line = String::new();
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    reader.read_line(&mut line).await?;
    let ans = line.trim().to_lowercase();
    Ok(matches!(ans.as_str(), "y" | "yes"))
}
