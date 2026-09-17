//! `dailybrief` entry point. Subcommands register here as milestones add them (`spec/r0.md` §4).
use clap::{Parser, Subcommand};
use dailybrief::commands;
use dailybrief::config::Env;

/// Personal daily-reading agent.
#[derive(Debug, Parser)]
#[command(name = "dailybrief", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Apply pending SQL migrations to <data_dir>/brief.db.
    Migrate,
    /// Ingest every enabled feed once (no agent) and print the report.
    Fetch,
    /// Serve the editor tools over stdio (spawned by the harness via mcp.json).
    Mcp,
}

/// Logs go to stderr in every command: stdout is the MCP protocol stream for `mcp` and the
/// report channel for the others.
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();
    let cli = Cli::parse();
    let env = Env::from_process()?;
    match cli.command {
        Command::Migrate => commands::migrate::run(&env, &mut std::io::stdout())?,
        Command::Fetch => {
            commands::fetch::run(&env, &mut std::io::stdout()).await?;
        }
        Command::Mcp => commands::mcp::run(&env).await?,
    }
    Ok(())
}
