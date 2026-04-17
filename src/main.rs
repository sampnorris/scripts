use anyhow::Result;
use clap::{Parser, Subcommand};

mod git;
mod ollama;
mod ui;

mod commands {
    pub mod commit;
    pub mod pr;
}

/// Ciri — local-LLM powered dev CLI.
#[derive(Parser)]
#[command(name = "ciri", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,

    /// Ollama model to use (overrides per-command default).
    #[arg(long, global = true)]
    model: Option<String>,

    /// Ollama base URL.
    #[arg(long, global = true, default_value = "http://localhost:11434")]
    host: String,
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
    match cli.cmd {
        Cmd::Commit(args) => commands::commit::run(args, &cli.host, cli.model.as_deref()).await,
        Cmd::Pr(args) => commands::pr::run(args, &cli.host, cli.model.as_deref()).await,
    }
}
