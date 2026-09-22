//! Retention (ADR 0013): run directories (transcript and `mcp.json`) and `run_events` older
//! than the window are removed; `runs` rows never are, so `/runs` keeps the whole history.
//! Nothing with `status = running` is touched. The directory goes before the rows, so a failed
//! removal leaves the page able to show what the file still has.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::core::time::to_iso;
use crate::db::{Db, DbError, repo};
use crate::harness::runner::run_dir;

#[derive(Debug, thiserror::Error)]
pub enum RetentionError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("cannot remove {path}: {source}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// One run the window no longer covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneItem {
    pub run_id: String,
    pub started_at: String,
    pub dir: PathBuf,
    pub dir_exists: bool,
    pub events: i64,
}

/// A run whose directory could not be removed; its rows are kept and it is retried next time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneFailure {
    pub run_id: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneReport {
    pub dry_run: bool,
    pub days: u32,
    pub cutoff: String,
    pub runs: usize,
    pub dirs_removed: usize,
    pub events_deleted: usize,
    pub items: Vec<PruneItem>,
    pub failed: Vec<PruneFailure>,
}

/// Run ids are minted by the runner (`YYYY-MM-DD-<8 hex>`); anything that could leave the
/// runs directory is refused here as a second line of defence.
pub fn id_is_a_plain_name(id: &str) -> bool {
    !id.is_empty() && !id.contains('/') && !id.contains('\\') && !id.starts_with('.')
}

/// The RFC 3339 instant before which runs fall out of the window.
pub fn cutoff(now: DateTime<Utc>, days: u32) -> String {
    to_iso(now - chrono::Duration::days(i64::from(days)))
}

/// What a prune would remove: finished runs started before the cutoff, with their directory
/// and event count. Never a running run, never a `runs` row.
pub async fn plan(
    db: &Db,
    data_dir: &Path,
    now: DateTime<Utc>,
    days: u32,
) -> Result<Vec<PruneItem>, RetentionError> {
    let before = cutoff(now, days);
    let data_dir = data_dir.to_path_buf();
    let items = db
        .call(move |conn| {
            let mut items = Vec::new();
            let runs_root = data_dir.join("runs");
            for run in repo::list_runs_started_before(conn, &before)? {
                let dir = run_dir(&data_dir, &run.id);
                if !id_is_a_plain_name(&run.id) || !dir.starts_with(&runs_root) {
                    tracing::warn!(run_id = %run.id, "prune: run id is not a plain directory name; skipped");
                    continue;
                }
                items.push(PruneItem {
                    dir_exists: dir.is_dir(),
                    dir,
                    events: repo::count_run_events(conn, &run.id)?,
                    run_id: run.id,
                    started_at: run.started_at,
                });
            }
            Ok(items)
        })
        .await?;
    Ok(items)
}

/// Removes what `plan` listed: the run directory first (transcript, `mcp.json`), then the
/// `run_events` rows. A directory that cannot be removed keeps its rows, is reported under
/// `failed`, and does not stop the other runs from being pruned; a directory that is already
/// gone is fine.
pub async fn prune(
    db: &Db,
    data_dir: &Path,
    now: DateTime<Utc>,
    days: u32,
    dry_run: bool,
) -> Result<PruneReport, RetentionError> {
    let items = plan(db, data_dir, now, days).await?;
    let mut report = PruneReport {
        dry_run,
        days,
        cutoff: cutoff(now, days),
        runs: items.len(),
        dirs_removed: 0,
        events_deleted: 0,
        items: items.clone(),
        failed: Vec::new(),
    };
    if dry_run {
        return Ok(report);
    }
    for item in items {
        let dir = item.dir.clone();
        let removed = tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(RetentionError::Remove { path: dir, source }),
        })
        .await
        .map_err(|e| RetentionError::Remove {
            path: item.dir.clone(),
            source: std::io::Error::other(e),
        });
        let removed = match removed {
            Ok(Ok(removed)) => removed,
            Ok(Err(e)) | Err(e) => {
                tracing::warn!(run_id = %item.run_id, error = %e, "prune: directory kept, rows kept");
                report.failed.push(PruneFailure {
                    run_id: item.run_id.clone(),
                    error: e.to_string(),
                });
                continue;
            }
        };
        if removed {
            report.dirs_removed += 1;
        }
        let id = item.run_id.clone();
        report.events_deleted += db
            .call(move |conn| repo::delete_run_events(conn, &id))
            .await?;
        tracing::info!(run_id = %item.run_id, events = item.events, removed_dir = removed, "pruned");
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::repo::{NewRun, Role, RunFinish, RunKind, RunStatus};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 21, 0, 0, 0).unwrap()
    }

    /// A run `age_days` old with a directory holding a transcript and two events.
    fn seed(db: &Db, data_dir: &Path, id: &str, age_days: i64, status: RunStatus, with_dir: bool) {
        let started = to_iso(now() - chrono::Duration::days(age_days));
        if with_dir {
            let dir = run_dir(data_dir, id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("transcript.jsonl"), "{}\n{}\n").unwrap();
            std::fs::write(dir.join("mcp.json"), "{}").unwrap();
        }
        db.with(|c| {
            repo::insert_run(
                c,
                &NewRun {
                    id: id.into(),
                    kind: RunKind::Scheduled,
                    role: Role::Editor,
                    harness: "claude-code".into(),
                    attempt: 1,
                    started_at: started.clone(),
                    transcript_path: Some(
                        run_dir(data_dir, id)
                            .join("transcript.jsonl")
                            .to_string_lossy()
                            .into_owned(),
                    ),
                },
            )?;
            if status != RunStatus::Running {
                repo::finish_run(
                    c,
                    id,
                    &RunFinish {
                        status,
                        ended_at: started,
                        turns: Some(1),
                        usage_json: None,
                        cost_usd: None,
                        session_id: None,
                        error: None,
                    },
                )?;
            }
            repo::append_run_event(c, id, 1, "system", "{}")?;
            repo::append_run_event(c, id, 2, "result", "{}")?;
            Ok(())
        })
        .unwrap();
    }

    fn rig() -> (tempfile::TempDir, Db) {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(&tmp.path().join("brief.db")).unwrap();
        (tmp, db)
    }

    fn counts(db: &Db) -> (i64, Vec<(String, i64)>) {
        db.with(|c| {
            let runs: i64 = c.query_row("SELECT count(*) FROM runs", [], |r| r.get(0))?;
            let mut stmt = c.prepare(
                "SELECT run_id, count(*) FROM run_events GROUP BY run_id ORDER BY run_id",
            )?;
            let per = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<(String, i64)>, _>>()?;
            Ok((runs, per))
        })
        .unwrap()
    }

    #[tokio::test]
    async fn prune_keeps_runs_rows_and_recent_runs() {
        let (tmp, db) = rig();
        seed(&db, tmp.path(), "r-030", 30, RunStatus::Success, true);
        seed(&db, tmp.path(), "r-061", 61, RunStatus::Failed, true);
        seed(&db, tmp.path(), "r-400", 400, RunStatus::Success, true);
        let report = prune(&db, tmp.path(), now(), 60, false).await.unwrap();
        assert_eq!(
            (report.runs, report.dirs_removed, report.events_deleted),
            (2, 2, 4)
        );
        assert_eq!(
            report
                .items
                .iter()
                .map(|i| i.run_id.as_str())
                .collect::<Vec<_>>(),
            ["r-400", "r-061"],
            "oldest first"
        );
        assert!(
            run_dir(tmp.path(), "r-030")
                .join("transcript.jsonl")
                .is_file()
        );
        assert!(!run_dir(tmp.path(), "r-061").exists());
        assert!(!run_dir(tmp.path(), "r-400").exists());
        let (runs, per) = counts(&db);
        assert_eq!(runs, 3, "runs rows are never pruned");
        assert_eq!(per, vec![("r-030".to_string(), 2)]);
    }

    #[tokio::test]
    async fn prune_dry_run_deletes_nothing() {
        let (tmp, db) = rig();
        seed(&db, tmp.path(), "r-061", 61, RunStatus::Success, true);
        let report = prune(&db, tmp.path(), now(), 60, true).await.unwrap();
        assert_eq!(report.runs, 1);
        assert!(report.dry_run);
        assert_eq!((report.dirs_removed, report.events_deleted), (0, 0));
        assert!(
            run_dir(tmp.path(), "r-061")
                .join("transcript.jsonl")
                .is_file()
        );
        assert_eq!(counts(&db).1, vec![("r-061".to_string(), 2)]);
    }

    #[tokio::test]
    async fn prune_skips_running_runs() {
        let (tmp, db) = rig();
        seed(&db, tmp.path(), "r-100-live", 100, RunStatus::Running, true);
        let report = prune(&db, tmp.path(), now(), 60, false).await.unwrap();
        assert_eq!(report.runs, 0);
        assert!(run_dir(tmp.path(), "r-100-live").exists());
    }

    #[tokio::test]
    async fn prune_tolerates_a_missing_directory() {
        let (tmp, db) = rig();
        seed(&db, tmp.path(), "r-090", 90, RunStatus::Success, false);
        let report = prune(&db, tmp.path(), now(), 60, false).await.unwrap();
        assert_eq!(
            (report.runs, report.dirs_removed, report.events_deleted),
            (1, 0, 2)
        );
        assert!(!report.items[0].dir_exists);
        assert_eq!(counts(&db), (1, vec![]));
    }

    #[test]
    fn cutoff_is_days_before_now() {
        assert_eq!(cutoff(now(), 60), "2026-07-23T00:00:00.000Z");
    }

    /// One directory that cannot be removed keeps its rows and is reported; the others are
    /// still pruned, so a single stuck run cannot stop retention for good.
    #[tokio::test]
    async fn prune_keeps_rows_when_a_directory_cannot_be_removed() {
        let (tmp, db) = rig();
        seed(&db, tmp.path(), "r-400", 400, RunStatus::Success, false);
        // A plain file where the run directory should be: remove_dir_all fails.
        std::fs::create_dir_all(tmp.path().join("runs")).unwrap();
        std::fs::write(run_dir(tmp.path(), "r-400"), "not a directory").unwrap();
        seed(&db, tmp.path(), "r-061", 61, RunStatus::Success, true);
        let report = prune(&db, tmp.path(), now(), 60, false).await.unwrap();
        assert_eq!(report.runs, 2);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].run_id, "r-400");
        assert_eq!((report.dirs_removed, report.events_deleted), (1, 2));
        assert!(!run_dir(tmp.path(), "r-061").exists());
        assert!(run_dir(tmp.path(), "r-400").is_file(), "left in place");
        let (runs, per) = counts(&db);
        assert_eq!(runs, 2);
        assert_eq!(
            per,
            vec![("r-400".to_string(), 2)],
            "the stuck run keeps its rows"
        );
    }

    #[test]
    fn run_ids_that_could_leave_the_runs_directory_are_refused() {
        for bad in ["", ".", "..", "../x", "a/b", "/data", ".hidden", "x\\y"] {
            assert!(!id_is_a_plain_name(bad), "{bad:?}");
        }
        assert!(id_is_a_plain_name("2026-09-22-7494d2ef"));
        assert!(id_is_a_plain_name("r-061"));
    }

    #[tokio::test]
    async fn plan_skips_run_ids_that_escape_the_runs_directory() {
        let (tmp, db) = rig();
        seed(&db, tmp.path(), "../escape", 400, RunStatus::Success, false);
        seed(&db, tmp.path(), "r-400", 400, RunStatus::Success, false);
        let items = plan(&db, tmp.path(), now(), 60).await.unwrap();
        assert_eq!(
            items.iter().map(|i| i.run_id.as_str()).collect::<Vec<_>>(),
            ["r-400"]
        );
    }
}
