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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let env = Env::from_process()?;
    match cli.command {
        Command::Migrate => commands::migrate::run(&env, &mut std::io::stdout())?,
    }
    Ok(())
}
