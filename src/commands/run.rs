//! `dailybrief run`: one on-demand run now, through the same code path as the scheduler and
//! `POST /run`. Prints the `RunSummary` as JSON; exit 0 success, 1 failed/killed, 3 locked.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use crate::config::{Env, HarnessName, load_all};
use crate::core::embed::embedder_for;
use crate::db::Db;
use crate::db::repo::RunKind;
use crate::harness::runner::{RunSummary, RunnerError};
use crate::harness::service_runner::{ServiceRunner, ServiceRunnerError, ServiceRunnerOptions};

use super::{CommandError, db_path};

#[derive(Debug, Clone)]
pub struct RunArgs {
    pub harness: Option<String>,
    pub kind: RunKind,
    pub prompt: Option<PathBuf>,
    pub schema: Option<PathBuf>,
    pub message: Option<String>,
    pub verify: bool,
    pub attempts: u32,
}

/// The process exit code for a summary.
pub fn exit_code(summary: &RunSummary) -> i32 {
    if summary.status == "success" { 0 } else { 1 }
}

pub const EXIT_LOCKED: i32 = 3;

pub async fn run(
    env: &Env,
    args: RunArgs,
    process_env: &HashMap<String, String>,
    out: &mut impl Write,
) -> Result<i32, CommandError> {
    let loaded = load_all(env)?;
    let harness = args
        .harness
        .unwrap_or_else(|| loaded.config.harness.default.as_str().to_string());
    if harness != HarnessName::ClaudeCode.as_str() {
        return Err(CommandError::Usage(format!(
            "harness '{harness}' is not available in R0 (only claude-code)"
        )));
    }
    for (what, path) in [("prompt", &args.prompt), ("schema", &args.schema)] {
        if let Some(p) = path
            && !p.exists()
        {
            return Err(CommandError::Usage(format!(
                "{what} file not found: {}",
                p.display()
            )));
        }
    }
    let db = Db::open(&db_path(&loaded.config))?;
    let runner = ServiceRunner::new(
        loaded.config.clone(),
        db,
        loaded.topics,
        embedder_for(&loaded.config),
        process_env,
        ServiceRunnerOptions {
            system_prompt_path: args.prompt,
            schema_path: args.schema,
            user_message: args.message,
            verify: args.verify,
            max_attempts: args.attempts.clamp(1, 2),
        },
    )
    .map_err(service_error)?;
    match runner.run(args.kind).await {
        Ok(summary) => {
            writeln!(out, "{}", serde_json::to_string_pretty(&summary)?)?;
            Ok(exit_code(&summary))
        }
        Err(ServiceRunnerError::Runner(RunnerError::Locked { held_by, since })) => {
            tracing::warn!(held_by, since, "run refused: lock held");
            writeln!(
                out,
                "{{\"error\":\"a run is already in progress\",\"heldBy\":{}}}",
                serde_json::to_string(&held_by)?
            )?;
            Ok(EXIT_LOCKED)
        }
        Err(e) => Err(service_error(e)),
    }
}

fn service_error(e: ServiceRunnerError) -> CommandError {
    match e {
        ServiceRunnerError::Env(env) => CommandError::Env(env),
        ServiceRunnerError::Runner(r) => CommandError::Runner(r),
        ServiceRunnerError::Db(d) => CommandError::Db(d),
        other => CommandError::Usage(other.to_string()),
    }
}
