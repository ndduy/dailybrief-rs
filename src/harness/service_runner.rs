//! What the CLI, the scheduler and `POST /run` share: the configured adapter built from the
//! process environment, the prompt and schema files, topic sync before each run, and digest
//! verification after it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::claude_code::{ClaudeCodeAdapter, EnvError, USER_MESSAGE};
use super::runner::{RunSummary, Runner, RunnerError, new_run_id};
use super::types::HarnessKind;
use crate::config::{Config, Topic};
use crate::core::embed::Embedder;
use crate::core::profile::sync_topics;
use crate::db::Db;
use crate::db::repo::RunKind;

#[derive(Debug, thiserror::Error)]
pub enum ServiceRunnerError {
    #[error(transparent)]
    Env(#[from] EnvError),
    #[error("cannot read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not valid JSON: {source}")]
    Schema {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Runner(#[from] RunnerError),
    #[error(transparent)]
    Db(#[from] crate::db::DbError),
    #[error(transparent)]
    Time(#[from] crate::core::time::TimeError),
}

pub struct ServiceRunnerOptions {
    pub system_prompt_path: Option<PathBuf>,
    pub schema_path: Option<PathBuf>,
    pub user_message: Option<String>,
    pub verify: bool,
    pub max_attempts: u32,
}

impl Default for ServiceRunnerOptions {
    fn default() -> Self {
        Self {
            system_prompt_path: None,
            schema_path: None,
            user_message: None,
            verify: true,
            max_attempts: 2,
        }
    }
}

pub struct ServiceRunner {
    runner: Runner,
    embedder: Arc<dyn Embedder>,
    topics: Vec<Topic>,
}

impl ServiceRunner {
    /// Builds the production runner: the claude-code adapter over the scrubbed process env.
    pub fn new(
        config: Config,
        db: Db,
        topics: Vec<Topic>,
        embedder: Arc<dyn Embedder>,
        process_env: &HashMap<String, String>,
        opts: ServiceRunnerOptions,
    ) -> Result<Self, ServiceRunnerError> {
        let adapter = ClaudeCodeAdapter::new(config.harness.claude_code.clone(), process_env)?;
        Self::with_harness(
            config,
            db,
            topics,
            embedder,
            HarnessKind::ClaudeCode(adapter),
            opts,
        )
    }

    /// The same runner over any harness (tests point it at the fake `claude`).
    pub fn with_harness(
        config: Config,
        db: Db,
        topics: Vec<Topic>,
        embedder: Arc<dyn Embedder>,
        harness: HarnessKind,
        opts: ServiceRunnerOptions,
    ) -> Result<Self, ServiceRunnerError> {
        let schema_path = opts
            .schema_path
            .unwrap_or_else(|| config.paths.digest_schema.clone());
        let schema_text =
            std::fs::read_to_string(&schema_path).map_err(|source| ServiceRunnerError::Read {
                path: schema_path.clone(),
                source,
            })?;
        let json_schema =
            serde_json::from_str(&schema_text).map_err(|source| ServiceRunnerError::Schema {
                path: schema_path,
                source,
            })?;
        let tz = crate::core::time::parse_tz(&config.service.timezone)?;
        let runner = Runner {
            tz,
            db,
            system_prompt_path: opts
                .system_prompt_path
                .unwrap_or_else(|| config.paths.editor_prompt.clone()),
            json_schema,
            user_message: opts
                .user_message
                .unwrap_or_else(|| USER_MESSAGE.to_string()),
            now: Utc::now,
            new_id: new_run_id,
            verify: opts.verify,
            max_attempts: opts.max_attempts,
            harness,
            config,
        };
        Ok(Self {
            runner,
            embedder,
            topics,
        })
    }

    /// Mirrors `topics.toml` into the database, then runs the harness.
    pub async fn run(&self, kind: RunKind) -> Result<RunSummary, ServiceRunnerError> {
        sync_topics(&self.runner.db, Arc::clone(&self.embedder), &self.topics).await?;
        Ok(self.runner.run(kind).await?)
    }

    /// The live holder of the run lock, if any (scheduled runs hold it without touching the
    /// web layer's `active` flag).
    pub async fn lock_holder(&self) -> Result<Option<String>, ServiceRunnerError> {
        Ok(self.runner.lock_holder().await?)
    }

    pub fn now(&self) -> DateTime<Utc> {
        (self.runner.now)()
    }
}
