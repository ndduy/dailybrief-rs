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
    /// Run the Editor once now (same code path as the scheduler and POST /run).
    Run {
        /// Harness to use (only claude-code in R0).
        #[arg(long)]
        harness: Option<String>,
        /// manual | scheduled
        #[arg(long, default_value = "manual")]
        kind: String,
        /// Attempts (1 or 2; 2 = retry once).
        #[arg(long, default_value_t = 2)]
        attempts: u32,
        /// Accept a harness success without a digest row (smoke runs only).
        #[arg(long)]
        no_verify: bool,
        /// System prompt file override.
        #[arg(long)]
        prompt: Option<std::path::PathBuf>,
        /// Output schema override.
        #[arg(long)]
        schema: Option<std::path::PathBuf>,
        /// User message override.
        #[arg(long)]
        message: Option<String>,
        /// Override harness.claude-code.max_turns for this run (the M2 gate uses 5).
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        max_turns: Option<u32>,
    },
    /// Run the Curator once now: read the week's feedback, write proposals for /curator.
    Curate {
        /// manual | scheduled
        #[arg(long, default_value = "manual")]
        kind: String,
        /// Attempts (1 or 2; 2 = retry once).
        #[arg(long, default_value_t = 2)]
        attempts: u32,
        /// Override harness.claude-code.max_turns for this run.
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
        max_turns: Option<u32>,
    },
    /// The PreToolUse hook body (spawned by Claude Code via the per-run settings.json).
    Hook {
        /// pre-tool-use
        event: String,
    },
    /// Serve the reading pages (and the scheduler) on service.bind:service.port.
    Serve,
    /// Re-embed every item and topic with the current model (first deployment).
    Reembed,
    /// Remove run directories and run_events older than the retention window; runs rows stay.
    Prune {
        /// Window in days (default: [retention].days, 60; never below 7).
        #[arg(long, value_parser = clap::value_parser!(u32).range(7..))]
        days: Option<u32>,
        /// List what would be removed without removing it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Egress scan of a run transcript; exit 1 on any finding.
    ScanTranscript {
        /// Path to transcript.jsonl
        path: std::path::PathBuf,
    },
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
        Command::Run {
            harness,
            kind,
            attempts,
            no_verify,
            prompt,
            schema,
            message,
            max_turns,
        } => {
            let kind = match kind.as_str() {
                "manual" => dailybrief::db::repo::RunKind::Manual,
                "scheduled" => dailybrief::db::repo::RunKind::Scheduled,
                other => anyhow::bail!("run: --kind must be manual or scheduled, got '{other}'"),
            };
            let process_env: std::collections::HashMap<String, String> = std::env::vars().collect();
            let code = commands::run::run(
                &env,
                commands::run::RunArgs {
                    harness,
                    kind,
                    role: dailybrief::db::repo::Role::Editor,
                    prompt,
                    schema,
                    message,
                    verify: !no_verify,
                    attempts,
                    max_turns,
                },
                &process_env,
                &mut std::io::stdout(),
            )
            .await?;
            std::process::exit(code);
        }
        Command::Curate {
            kind,
            attempts,
            max_turns,
        } => {
            let kind = match kind.as_str() {
                "manual" => dailybrief::db::repo::RunKind::Manual,
                "scheduled" => dailybrief::db::repo::RunKind::Scheduled,
                other => anyhow::bail!("curate: --kind must be manual or scheduled, got '{other}'"),
            };
            let process_env: std::collections::HashMap<String, String> = std::env::vars().collect();
            let code = commands::curate::run(
                &env,
                kind,
                attempts,
                max_turns,
                &process_env,
                &mut std::io::stdout(),
            )
            .await?;
            std::process::exit(code);
        }
        Command::Hook { event } => {
            let cwd = std::env::current_dir()?;
            let code =
                commands::hook::run(&event, &mut std::io::stdin(), &cwd, &mut std::io::stderr())?;
            std::process::exit(code);
        }
        Command::Reembed => commands::reembed::run(&env, &mut std::io::stdout()).await?,
        Command::Serve => {
            let process_env: std::collections::HashMap<String, String> = std::env::vars().collect();
            commands::serve::run(&env, &process_env).await?;
        }
        Command::Prune { days, dry_run } => {
            let code = commands::prune::run(&env, days, dry_run, &mut std::io::stdout()).await?;
            std::process::exit(code);
        }
        Command::ScanTranscript { path } => {
            let code = commands::scan_transcript::run(&env, &path, &mut std::io::stdout()).await?;
            std::process::exit(code);
        }
    }
    Ok(())
}
