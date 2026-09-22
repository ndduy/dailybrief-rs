//! `dailybrief curate`: one Curator run now (`spec/m3.md` §3), through the same runner as
//! `run` with the Curator's role: its prompt, schema, tool set and verification. Prints the
//! `RunSummary` as JSON; exit 0 success, 1 failed/killed, 3 locked.

use std::collections::HashMap;
use std::io::Write;

use crate::config::Env;
use crate::db::repo::{Role, RunKind};

use super::CommandError;
use super::run::{RunArgs, run as run_verb};

pub async fn run(
    env: &Env,
    kind: RunKind,
    attempts: u32,
    max_turns: Option<u32>,
    process_env: &HashMap<String, String>,
    out: &mut impl Write,
) -> Result<i32, CommandError> {
    run_verb(
        env,
        RunArgs {
            harness: None,
            kind,
            role: Role::Curator,
            prompt: None,
            schema: None,
            message: None,
            verify: true,
            attempts,
            max_turns,
        },
        process_env,
        out,
    )
    .await
}
