//! The run lifecycle (`SPEC.md` §3 "Runner policy, precisely"): lock, one or two attempts with
//! fresh run ids and directories, the rendered `mcp.json`, every stdout line to the transcript
//! file and `run_events`, outcome classification with digest verification, and the `runs` row.
//! No deterministic fallback, ever.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use super::claude_code::event_type;
use super::types::{HarnessKind, HarnessRequest, RunOutcome};
use super::verify::verify_digest_outcome;
use crate::config::Config;
use chrono_tz::Tz;

use crate::core::time::{date_in_zone, days_ago_iso, to_iso};
use crate::db::repo::{self, LockResult, NewRun, RunFinish, RunKind, RunStatus};
use crate::db::{Db, DbError};
use crate::editor::mcp_config::{McpConfigError, McpConfigVars, render};

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("a run is already in progress ({held_by}, since {since})")]
    Locked { held_by: String, since: String },
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("run directory: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    McpConfig(#[from] McpConfigError),
    #[error("no attempt ran")]
    NoAttempt,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub status: String,
    pub attempts: u32,
    pub run_ids: Vec<String>,
    pub final_run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
}

/// `YYYY-MM-DD-<8 hex>` with the date in the service timezone (`SPEC.md` §3): the 06:30 run in
/// Ho Chi Minh fires at 23:30 UTC and must not be filed under the previous day.
pub fn new_run_id(now: DateTime<Utc>, tz: Tz) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{}-{}", date_in_zone(now, tz), &hex[..8])
}

pub fn run_dir(data_dir: &Path, run_id: &str) -> PathBuf {
    data_dir.join("runs").join(run_id)
}

pub struct Runner {
    pub db: Db,
    pub config: Config,
    pub harness: HarnessKind,
    pub system_prompt_path: PathBuf,
    pub json_schema: serde_json::Value,
    pub user_message: String,
    pub now: fn() -> DateTime<Utc>,
    pub new_id: fn(DateTime<Utc>, Tz) -> String,
    /// The service timezone; run ids and digest dates are local dates.
    pub tz: Tz,
    /// `false` accepts a harness success without a digest row (smoke runs only).
    pub verify: bool,
    /// 2 = retry once (the spec default); 1 for smoke runs.
    pub max_attempts: u32,
}

struct AttemptEnd {
    run_id: String,
    status: RunStatus,
    error: Option<String>,
    outcome: RunOutcome,
}

impl Runner {
    /// Holders older than `2 × wall clock + 5 min` are stale and may be taken over.
    fn stale_before(&self, now: DateTime<Utc>) -> String {
        let stale_minutes = 2 * self.config.harness.claude_code.wall_clock_minutes + 5;
        to_iso(now - chrono::Duration::minutes(i64::from(stale_minutes)))
    }

    /// The live holder of the run lock, if any (a stale holder counts as none).
    pub async fn lock_holder(&self) -> Result<Option<String>, RunnerError> {
        let stale_before = self.stale_before((self.now)());
        let lock = self.db.call(|conn| repo::current_lock(conn)).await?;
        Ok(lock
            .filter(|(_, acquired_at)| acquired_at.as_str() >= stale_before.as_str())
            .map(|(run_id, _)| run_id))
    }

    /// Runs the harness, retrying once from a fresh context; refuses when a live run holds the lock.
    pub async fn run(&self, kind: RunKind) -> Result<RunSummary, RunnerError> {
        let started = (self.now)();
        let stale_before = self.stale_before(started);
        let holder = format!("{}@{}", kind.as_str(), to_iso(started));
        let now_iso = to_iso(started);
        let lock = self
            .db
            .call(move |conn| repo::try_acquire_lock(conn, &holder, &now_iso, &stale_before))
            .await?;
        if let LockResult::Held {
            run_id,
            acquired_at,
        } = lock
        {
            return Err(RunnerError::Locked {
                held_by: run_id,
                since: acquired_at,
            });
        }
        let result = self.attempts(kind).await;
        self.db.call(|conn| repo::release_lock(conn)).await?;
        result
    }

    async fn attempts(&self, kind: RunKind) -> Result<RunSummary, RunnerError> {
        let mut run_ids = Vec::new();
        let mut last: Option<AttemptEnd> = None;
        for n in 1..=self.max_attempts.max(1) {
            let end = self.attempt(kind, n).await?;
            run_ids.push(end.run_id.clone());
            let success = end.status == RunStatus::Success;
            if !success {
                tracing::warn!(run_id = %end.run_id, attempt = n, error = ?end.error, "attempt failed");
            }
            last = Some(end);
            if success {
                break;
            }
        }
        let last = last.ok_or(RunnerError::NoAttempt)?;
        let structured_output = match &last.outcome {
            RunOutcome::Success { result, .. } if last.status == RunStatus::Success => {
                result.structured_output.clone()
            }
            _ => None,
        };
        Ok(RunSummary {
            status: last.status.as_str().to_string(),
            attempts: run_ids.len() as u32,
            run_ids,
            final_run_id: last.run_id,
            error: last.error,
            structured_output,
        })
    }

    async fn attempt(&self, kind: RunKind, attempt_no: u32) -> Result<AttemptEnd, RunnerError> {
        let started = (self.now)();
        let run_id = (self.new_id)(started, self.tz);
        let dir = run_dir(&self.config.paths.data_dir, &run_id);
        tokio::fs::create_dir_all(&dir).await?;
        let transcript_path = dir.join("transcript.jsonl");
        let mcp_config_path = dir.join("mcp.json");
        let template = tokio::fs::read_to_string(&self.config.paths.mcp_template).await?;
        let command = std::env::current_exe()?;
        let rendered = render(
            &template,
            &McpConfigVars {
                command: &command,
                args: &self.config.harness.mcp_args,
                run_id: &run_id,
                config_path: &self.config.paths.config,
                data_dir: &self.config.paths.data_dir,
            },
        )?;
        tokio::fs::write(&mcp_config_path, rendered).await?;
        tokio::fs::write(&transcript_path, b"").await?;
        {
            let row = NewRun {
                id: run_id.clone(),
                kind,
                harness: self.harness.name().to_string(),
                attempt: i64::from(attempt_no),
                started_at: to_iso(started),
                transcript_path: Some(transcript_path.to_string_lossy().into_owned()),
            };
            self.db
                .call(move |conn| repo::insert_run(conn, &row))
                .await?;
        }
        tracing::info!(run_id = %run_id, kind = kind.as_str(), attempt = attempt_no, "run started");

        // Every raw line goes to the transcript file and run_events through one writer task, so
        // the adapter's synchronous hook never touches the runtime or the database directly.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, String)>();
        let writer = {
            let db = self.db.clone();
            let run_id = run_id.clone();
            let transcript_path = transcript_path.clone();
            tokio::spawn(async move {
                let mut file = tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(&transcript_path)
                    .await?;
                while let Some((seq, raw)) = rx.recv().await {
                    file.write_all(raw.as_bytes()).await?;
                    file.write_all(b"\n").await?;
                    let (id, kind) = (run_id.clone(), event_type(&raw));
                    // The file is the source of truth (`SPEC.md` §3): a database hiccup (a busy
                    // writer, a dropped table) must not lose the rest of the transcript.
                    if let Err(e) = db
                        .call(move |conn| {
                            repo::append_run_event(conn, &id, seq as i64, &kind, &raw)
                        })
                        .await
                    {
                        tracing::error!(run_id = %run_id, seq, error = %e, "run_events insert failed; transcript file continues");
                    }
                }
                file.flush().await?;
                Ok::<(), std::io::Error>(())
            })
        };
        let req = HarnessRequest {
            run_id: run_id.clone(),
            cwd: dir.clone(),
            mcp_config_path,
            system_prompt_path: self.system_prompt_path.clone(),
            user_message: self.user_message.clone(),
            json_schema: self.json_schema.clone(),
        };
        let outcome = self
            .harness
            .run(&req, |raw, seq| {
                let _ = tx.send((seq, raw.to_string()));
            })
            .await;
        drop(tx);
        if let Ok(Err(e)) = writer.await {
            tracing::error!(run_id = %run_id, error = %e, "transcript writer failed");
        }

        let (status, error) = self.classify(&run_id, &outcome).await?;
        let result = outcome.result();
        let finish = RunFinish {
            status,
            ended_at: to_iso((self.now)()),
            turns: result.and_then(|r| r.num_turns).map(|t| t as i64),
            usage_json: result
                .and_then(|r| r.usage.as_ref())
                .map(ToString::to_string),
            cost_usd: result.and_then(|r| r.total_cost_usd),
            session_id: result.and_then(|r| r.session_id.clone()),
            error: error.clone(),
        };
        {
            let id = run_id.clone();
            self.db
                .call(move |conn| repo::finish_run(conn, &id, &finish))
                .await?;
        }
        tracing::info!(run_id = %run_id, status = status.as_str(), error = ?error, "run finished");
        Ok(AttemptEnd {
            run_id,
            status,
            error,
            outcome,
        })
    }

    async fn classify(
        &self,
        run_id: &str,
        outcome: &RunOutcome,
    ) -> Result<(RunStatus, Option<String>), RunnerError> {
        Ok(match outcome {
            RunOutcome::Killed { message, .. } => (RunStatus::Killed, Some(message.clone())),
            RunOutcome::Failed { message, .. } => (RunStatus::Failed, Some(message.clone())),
            RunOutcome::Success { .. } if !self.verify => (RunStatus::Success, None),
            RunOutcome::Success { .. } => {
                let (id, outcome) = (run_id.to_string(), outcome.clone());
                match self
                    .db
                    .call(move |conn| verify_digest_outcome(conn, &id, &outcome))
                    .await?
                {
                    Ok(()) => (RunStatus::Success, None),
                    Err(reason) => (RunStatus::Failed, Some(reason)),
                }
            }
        })
    }
}

/// The stored-form timestamp `days` before `now` (re-exported for the web layer's manual-run cap).
pub fn since_days(now: DateTime<Utc>, days: u32) -> String {
    days_ago_iso(now, days)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn run_id_uses_the_service_local_date() {
        let tz: Tz = "Asia/Ho_Chi_Minh".parse().unwrap();
        // 23:30 UTC on the 21st is 06:30 on the 22nd in Ho Chi Minh: the scheduled run.
        let at = Utc.with_ymd_and_hms(2026, 9, 21, 23, 30, 0).unwrap();
        let id = new_run_id(at, tz);
        assert!(id.starts_with("2026-09-22-"), "{id}");
        assert_eq!(id.len(), "2026-09-22-".len() + 8);
    }
}
