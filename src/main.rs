//! `dailybrief` entry point. Subcommands register here as milestones add them (`spec/r0.md` §4).
use clap::Parser;

/// Personal daily-reading agent.
#[derive(Debug, Parser)]
#[command(name = "dailybrief", version, about)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    Ok(())
}
