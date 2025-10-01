use tracing_subscriber::util::SubscriberInitExt;

use clap::{Parser, Subcommand};
use tracing::Level;

mod chat;
mod config;
mod diff;
mod edits;
mod fsutil;
mod llm;

#[derive(Parser)]
#[command(
    name = "smol",
    version,
    about = "Smol CLI — a lightweight coding agent for your terminal"
)]
struct Cli {
    /// Increase verbosity (-v, -vv)
    #[arg(short, long, action=clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start interactive chat REPL
    Chat {
        /// Model override, e.g. openai/gpt-4o-mini
        #[arg(long)]
        model: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let level = match cli.verbose {
        0 => Level::INFO,
        1 => Level::DEBUG,
        _ => Level::TRACE,
    };
    tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_max_level(level)
        .finish()
        .init();

    match cli.cmd {
        Commands::Chat { model } => chat::run(model).await?,
    }

    Ok(())
}
