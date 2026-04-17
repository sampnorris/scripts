use anyhow::Result;
use clap::{Parser, Subcommand};

mod git;
mod llm;
mod ui;

mod commands {
    pub mod commit;
    pub mod pr;
}

use llm::Mode;

/// Ciri — local/online-LLM powered dev CLI.
#[derive(Parser)]
#[command(name = "ciri", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,

    /// Override the model for the active backend.
    #[arg(long, global = true)]
    model: Option<String>,

    /// Ollama base URL (used when offline).
    #[arg(long, global = true, default_value = "http://localhost:11434")]
    host: String,

    /// Force the offline (Ollama) backend.
    #[arg(long, global = true, conflicts_with = "online")]
    offline: bool,

    /// Force the online (pi / OpenAI Codex) backend.
    #[arg(long, global = true)]
    online: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate a Conventional Commit message from staged changes and commit.
    Commit(commands::commit::Args),
    /// Open a pull request with an AI-generated title + body.
    Pr(commands::pr::Args),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mode = if cli.offline {
        Mode::ForceOffline
    } else if cli.online {
        Mode::ForceOnline
    } else {
        Mode::Auto
    };
    match cli.cmd {
        Cmd::Commit(args) => {
            commands::commit::run(args, &cli.host, cli.model.as_deref(), mode).await
        }
        Cmd::Pr(args) => commands::pr::run(args, &cli.host, cli.model.as_deref(), mode).await,
    }
}
