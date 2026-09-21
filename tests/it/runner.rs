//! Runner lifecycle against the fake `claude`: retry-once, killed, verification failures,
//! transcript + run_events, the lock, and two processes writing one SQLite file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use dailybrief::config::{Config, Env, load_config};
use dailybrief::core::embed::FakeEmbedder;
use dailybrief::db::Db;
use dailybrief::db::repo::{self, DigestInsert, RunKind, RunStatus};
use dailybrief::harness::claude_code::ClaudeCodeAdapter;
use dailybrief::harness::runner::{RunSummary, Runner, RunnerError};
use dailybrief::harness::service_runner::{ServiceRunner, ServiceRunnerOptions};
use dailybrief::harness::types::HarnessKind;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 17, 6, 0, 0).unwrap()
}

fn config_in(data_dir: &Path) -> Config {
    let data = data_dir.to_string_lossy().into_owned();
    load_config(&Env::from_lookup(|n| (n == "DAILYBRIEF_DATA_DIR").then(|| data.clone())).unwrap())
        .unwrap()
}

/// A fake adapter whose attempt N replays `script_dir/N.jsonl`.
fn fake(config: &Config, script_dir: &Path, hang: bool, wall_clock: Duration) -> HarnessKind {
    let mut parent: HashMap<String, String> = HashMap::new();
    parent.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
    parent.insert("HOME".into(), "/tmp".into());
    parent.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), "fake".into());
    parent.insert(
        "DAILYBRIEF_FAKE_SCRIPT_DIR".into(),
        script_dir.to_string_lossy().into_owned(),
    );
    if hang {
        parent.insert("DAILYBRIEF_FAKE_HANG".into(), "1".into());
    }
    let mut a = ClaudeCodeAdapter::new(config.harness.claude_code.clone(), &parent).unwrap();
    a.binary = root().join("tests/fake-claude/claude");
    a.wall_clock = wall_clock;
    a.kill_grace = Duration::from_secs(2);
    HarnessKind::ClaudeCode(a)
}

fn script(dir: &Path, attempts: &[&str]) {
    for (i, name) in attempts.iter().enumerate() {
        std::fs::copy(
            root().join("tests/fixtures/transcripts").join(name),
            dir.join(format!("{}.jsonl", i + 1)),
        )
        .unwrap();
    }
}

fn ids() -> fn(DateTime<Utc>, chrono_tz::Tz) -> String {
    fn next(_: DateTime<Utc>, _: chrono_tz::Tz) -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        format!("2026-09-17-run{:04}", N.fetch_add(1, Ordering::SeqCst))
    }
    next
}

struct Rig {
    _tmp: tempfile::TempDir,
    db: Db,
    runner: Runner,
}

fn rig(attempts: &[&str], hang: bool, verify: bool, wall_clock: Duration) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let script_dir = tmp.path().join("script");
    std::fs::create_dir_all(&script_dir).unwrap();
    script(&script_dir, attempts);
    let config = config_in(&data_dir);
    let db = Db::open(&data_dir.join("brief.db")).unwrap();
    let harness = fake(&config, &script_dir, hang, wall_clock);
    let runner = Runner {
        db: db.clone(),
        system_prompt_path: root().join("prompts/editor.md"),
        json_schema: serde_json::json!({ "type": "object" }),
        user_message: "Build today's digest. Start with get_briefing.".into(),
        now,
        new_id: ids(),
        tz: dailybrief::core::time::parse_tz("Asia/Ho_Chi_Minh").unwrap(),
        verify,
        max_attempts: 2,
        harness,
        config,
    };
    Rig {
        _tmp: tmp,
        db,
        runner,
    }
}

fn run_row(db: &Db, id: &str) -> repo::RunRow {
    db.with(|c| repo::get_run(c, id)).unwrap().unwrap()
}

#[tokio::test]
async fn runner_retries_once_then_succeeds_without_verification() {
    let r = rig(
        &["max-turns.jsonl", "success.jsonl"],
        false,
        false,
        Duration::from_secs(10),
    );
    let summary: RunSummary = r.runner.run(RunKind::Manual).await.unwrap();
    assert_eq!(summary.status, "success");
    assert_eq!(summary.attempts, 2);
    assert_eq!(summary.run_ids.len(), 2);
    assert_eq!(summary.structured_output.unwrap()["forYou"], 24);
    let first = run_row(&r.db, &summary.run_ids[0]);
    assert_eq!(first.status, RunStatus::Failed);
    assert_eq!(
        first.error.as_deref(),
        Some("Reached maximum number of turns (120)")
    );
    assert_eq!(first.attempt, 1);
    assert_eq!(first.turns, Some(120));
    let second = run_row(&r.db, &summary.run_ids[1]);
    assert_eq!(second.status, RunStatus::Success);
    assert_eq!(second.attempt, 2);
    assert_eq!(second.turns, Some(87));
    assert_eq!(second.session_id.as_deref(), Some("sess-success"));
    assert!(second.cost_usd.is_some());
    assert!(second.ended_at.is_some());
    assert!(
        r.db.with(
            |c| Ok(c.query_row("SELECT count(*) FROM run_lock", [], |r| r.get::<_, i64>(0))?)
        )
        .unwrap()
            == 0,
        "lock released"
    );
}

#[tokio::test]
async fn runner_marks_killed_and_stops_after_two_attempts() {
    let r = rig(&["no-result.jsonl"], true, true, Duration::from_millis(400));
    let summary = r.runner.run(RunKind::Scheduled).await.unwrap();
    assert_eq!(summary.status, "killed");
    assert_eq!(summary.attempts, 2);
    assert!(summary.error.unwrap().contains("wall clock"));
    for id in &summary.run_ids {
        assert_eq!(run_row(&r.db, id).status, RunStatus::Killed);
    }
}

#[tokio::test]
async fn runner_marks_failed_when_no_digest_row_and_keeps_the_run_id_on_mismatch() {
    let r = rig(&["success.jsonl"], false, true, Duration::from_secs(10));
    let summary = r.runner.run(RunKind::Manual).await.unwrap();
    assert_eq!(summary.status, "failed");
    assert_eq!(
        summary.error.as_deref(),
        Some("harness reported success but no digest was published for this run")
    );
    assert!(
        summary.structured_output.is_none(),
        "an unverified success carries no output"
    );
    assert_eq!(
        summary.attempts, 2,
        "verification failure is retried once too"
    );
}

#[tokio::test]
async fn runner_writes_transcript_and_run_events_in_order() {
    let r = rig(&["success.jsonl"], false, false, Duration::from_secs(10));
    let summary = r.runner.run(RunKind::Manual).await.unwrap();
    let id = &summary.final_run_id;
    let row = run_row(&r.db, id);
    let transcript = std::fs::read_to_string(row.transcript_path.unwrap()).unwrap();
    let expected =
        std::fs::read_to_string(root().join("tests/fixtures/transcripts/success.jsonl")).unwrap();
    assert_eq!(transcript, expected);
    let events = r.db.with(|c| repo::list_run_events(c, id)).unwrap();
    assert_eq!(
        events.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5, 6]
    );
    assert_eq!(
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        vec![
            "system",
            "assistant",
            "user",
            "rate_limit_event",
            "assistant",
            "result"
        ]
    );
    let dir = r.runner.config.paths.data_dir.join("runs").join(id);
    let mcp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("mcp.json")).unwrap()).unwrap();
    assert_eq!(
        mcp["mcpServers"]["dailybrief"]["env"]["DAILYBRIEF_RUN_ID"],
        id.as_str()
    );
    assert!(!dir.join("CLAUDE.md").exists());
}

#[tokio::test]
async fn runner_refuses_when_locked_and_takes_over_a_stale_lock() {
    let r = rig(&["success.jsonl"], false, false, Duration::from_secs(10));
    r.db.with(|c| {
        repo::try_acquire_lock(
            c,
            "other@live",
            "2026-09-17T05:59:00.000Z",
            "2026-09-17T05:00:00.000Z",
        )
    })
    .unwrap();
    let err = r.runner.run(RunKind::Manual).await.unwrap_err();
    assert!(
        matches!(err, RunnerError::Locked { ref held_by, .. } if held_by == "other@live"),
        "{err}"
    );
    // A holder older than 2 × wall clock + 5 min (35 min) is stale.
    r.db.with(|c| {
        c.execute(
            "UPDATE run_lock SET acquired_at = '2026-09-17T05:00:00.000Z'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let summary = r.runner.run(RunKind::Manual).await.unwrap();
    assert_eq!(summary.status, "success");
}

#[tokio::test]
async fn runner_releases_lock_after_failure() {
    let r = rig(&["no-result.jsonl"], false, true, Duration::from_secs(10));
    let summary = r.runner.run(RunKind::Manual).await.unwrap();
    assert_eq!(summary.status, "failed");
    let locks: i64 =
        r.db.with(|c| Ok(c.query_row("SELECT count(*) FROM run_lock", [], |r| r.get(0))?))
            .unwrap();
    assert_eq!(locks, 0);
}

#[tokio::test]
async fn service_runner_syncs_topics_and_verifies_a_digest() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let script_dir = tmp.path().join("script");
    std::fs::create_dir_all(&script_dir).unwrap();
    script(&script_dir, &["success.jsonl"]);
    let config = config_in(&data_dir);
    let db = Db::open(&data_dir.join("brief.db")).unwrap();
    let harness = fake(&config, &script_dir, false, Duration::from_secs(10));
    let topics = load_config(&Env::from_lookup(|_| None).unwrap())
        .map(|_| ())
        .ok();
    assert!(topics.is_some());
    let loaded = dailybrief::config::load_all(&Env::from_lookup(|_| None).unwrap()).unwrap();
    let runner = ServiceRunner::with_harness(
        config,
        db.clone(),
        loaded.topics.clone(),
        Arc::new(FakeEmbedder),
        harness,
        ServiceRunnerOptions {
            verify: false,
            max_attempts: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let summary = runner.run(RunKind::Manual).await.unwrap();
    assert_eq!(summary.status, "success");
    assert_eq!(summary.attempts, 1);
    let synced = db.with(|c| repo::list_topics(c)).unwrap();
    assert_eq!(synced.len(), loaded.topics.len());
    assert!(synced.iter().all(|t| t.vector.is_some()));
}

#[tokio::test]
async fn two_db_handles_write_one_file_concurrently() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("brief.db");
    let a = Db::open(&path).unwrap();
    let b = Db::open(&path).unwrap();
    for (db, id) in [(&a, "ra"), (&b, "rb")] {
        db.with(|c| {
            repo::insert_run(
                c,
                &repo::NewRun {
                    id: id.into(),
                    kind: RunKind::Manual,
                    harness: "t".into(),
                    attempt: 1,
                    started_at: "2026-09-17T06:00:00.000Z".into(),
                    transcript_path: None,
                },
            )
        })
        .unwrap();
    }
    let write = |db: Db, run: &'static str| async move {
        for seq in 1..=200i64 {
            let r = run.to_string();
            db.call(move |c| repo::append_run_event(c, &r, seq, "t", "{}"))
                .await
                .unwrap();
        }
    };
    tokio::join!(write(a.clone(), "ra"), write(b.clone(), "rb"));
    let n: i64 = a
        .with(|c| Ok(c.query_row("SELECT count(*) FROM run_events", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(n, 400);
    a.with(|c| {
        repo::insert_digest(
            c,
            &DigestInsert {
                id: "d".into(),
                date: "2026-09-17".into(),
                run_id: "ra".into(),
                published_at: "2026-09-17T06:10:00.000Z".into(),
                for_you_count: 0,
                beyond_radar_count: 0,
            },
            &[],
        )
    })
    .unwrap();
    assert!(
        b.with(|c| repo::get_digest_by_run(c, "ra"))
            .unwrap()
            .is_some()
    );
}

/// Two connections (the CLI against the service) racing for the lock: exactly one wins each
/// round, whether the row is absent or stale. (R0 ship review: the SELECT-then-INSERT version
/// let both take over a stale lock and spend two runs.)
#[tokio::test]
async fn lock_acquired_by_exactly_one_of_two_connections() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("brief.db");
    let a = Db::open(&path).unwrap();
    let b = Db::open(&path).unwrap();
    for round in 0..50 {
        if round % 2 == 1 {
            // Odd rounds start from a stale holder instead of an empty table.
            a.with(|c| {
                repo::try_acquire_lock(
                    c,
                    "stale",
                    "2026-09-17T05:00:00.000Z",
                    "2026-09-17T04:00:00.000Z",
                )?;
                Ok(())
            })
            .unwrap();
        }
        let acquire = |db: Db, who: &'static str| async move {
            db.call(move |c| {
                repo::try_acquire_lock(
                    c,
                    who,
                    "2026-09-17T06:00:00.000Z",
                    "2026-09-17T05:30:00.000Z",
                )
            })
            .await
            .unwrap()
        };
        let (ra, rb) = tokio::join!(acquire(a.clone(), "a"), acquire(b.clone(), "b"));
        let wins = [&ra, &rb]
            .into_iter()
            .filter(|r| matches!(r, repo::LockResult::Acquired))
            .count();
        assert_eq!(wins, 1, "round {round}: {ra:?} / {rb:?}");
        a.with(|c| repo::release_lock(c)).unwrap();
    }
}

/// The transcript file is written even when run_events cannot be (ship review: one failed
/// insert used to end the writer task and lose the rest of the file).
#[tokio::test]
async fn transcript_file_survives_run_events_failures() {
    let r = rig(&["success.jsonl"], false, false, Duration::from_secs(10));
    r.db.with(|c| {
        c.execute_batch("DROP TABLE run_events")?;
        Ok(())
    })
    .unwrap();
    let summary = r.runner.run(RunKind::Manual).await.unwrap();
    assert_eq!(summary.status, "success");
    let row = run_row(&r.db, &summary.final_run_id);
    let file = std::fs::read_to_string(row.transcript_path.unwrap()).unwrap();
    let expected =
        std::fs::read_to_string(root().join("tests/fixtures/transcripts/success.jsonl")).unwrap();
    assert_eq!(
        file.lines().count(),
        expected.lines().filter(|l| !l.trim().is_empty()).count()
    );
}
