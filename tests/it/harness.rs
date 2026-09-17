//! The claude-code adapter against the fake `claude` (`tests/fake-claude/claude`): every outcome
//! shape, the wall clock with SIGTERM before SIGKILL, stderr draining, and line delivery order.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use dailybrief::config::{Env, load_config};
use dailybrief::harness::claude_code::{ClaudeCodeAdapter, USER_MESSAGE};
use dailybrief::harness::types::{FailReason, HarnessKind, HarnessRequest, RunOutcome};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(name: &str) -> PathBuf {
    root().join("tests/fixtures/transcripts").join(name)
}

fn request(cwd: &Path) -> HarnessRequest {
    HarnessRequest {
        run_id: "2026-09-17-fake".into(),
        cwd: cwd.to_path_buf(),
        mcp_config_path: cwd.join("mcp.json"),
        system_prompt_path: root().join("prompts/editor.md"),
        user_message: USER_MESSAGE.into(),
        json_schema: serde_json::json!({ "type": "object" }),
    }
}

/// An adapter pointed at the fake, with the given DAILYBRIEF_FAKE_* knobs and a short wall clock.
fn adapter(knobs: &[(&str, &str)], wall_clock: Duration) -> HarnessKind {
    let settings = load_config(&Env::from_lookup(|_| None).unwrap())
        .unwrap()
        .harness
        .claude_code;
    let mut parent: HashMap<String, String> = HashMap::new();
    parent.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
    parent.insert("HOME".into(), "/tmp".into());
    parent.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), "fake-token".into());
    parent.insert("SHOULD_NOT_LEAK".into(), "1".into());
    for (k, v) in knobs {
        parent.insert((*k).to_string(), (*v).to_string());
    }
    let mut a = ClaudeCodeAdapter::new(settings, &parent).unwrap();
    a.binary = root().join("tests/fake-claude/claude");
    a.wall_clock = wall_clock;
    a.kill_grace = Duration::from_secs(2);
    HarnessKind::ClaudeCode(a)
}

async fn run_with(
    knobs: &[(&str, &str)],
    wall_clock: Duration,
) -> (RunOutcome, Vec<(u64, String)>) {
    let dir = tempfile::tempdir().unwrap();
    let mut lines = Vec::new();
    let outcome = adapter(knobs, wall_clock)
        .run(&request(dir.path()), |raw, seq| {
            lines.push((seq, raw.to_string()))
        })
        .await;
    (outcome, lines)
}

#[tokio::test]
async fn run_success_transcript() {
    let t = fixture("success.jsonl");
    let (outcome, lines) = run_with(
        &[("DAILYBRIEF_FAKE_TRANSCRIPT", t.to_str().unwrap())],
        Duration::from_secs(10),
    )
    .await;
    match outcome {
        RunOutcome::Success { result, init } => {
            assert_eq!(result.num_turns, Some(87));
            assert_eq!(result.structured_output.unwrap()["beyondRadar"], 6);
            assert_eq!(init.unwrap().mcp_servers[0].name, "dailybrief");
        }
        other => panic!("expected success, got {other:?}"),
    }
    assert_eq!(lines.len(), 6);
}

#[tokio::test]
async fn run_error_max_turns() {
    let t = fixture("max-turns.jsonl");
    let (outcome, _) = run_with(
        &[("DAILYBRIEF_FAKE_TRANSCRIPT", t.to_str().unwrap())],
        Duration::from_secs(10),
    )
    .await;
    match outcome {
        RunOutcome::Failed {
            reason,
            message,
            result,
            init,
            exit_code,
        } => {
            assert_eq!(reason, FailReason::MaxTurns);
            assert_eq!(message, "Reached maximum number of turns (120)");
            assert_eq!(result.unwrap().num_turns, Some(120));
            assert!(init.is_some());
            assert_eq!(exit_code, Some(0));
        }
        other => panic!("expected failed, got {other:?}"),
    }
}

#[tokio::test]
async fn run_hang_is_killed_by_wall_clock_with_sigterm_first() {
    let t = fixture("no-result.jsonl");
    let marker = tempfile::tempdir().unwrap();
    let marker_path = marker.path().join("term");
    let (outcome, lines) = run_with(
        &[
            ("DAILYBRIEF_FAKE_TRANSCRIPT", t.to_str().unwrap()),
            ("DAILYBRIEF_FAKE_HANG", "1"),
            ("DAILYBRIEF_FAKE_TERM_MARKER", marker_path.to_str().unwrap()),
        ],
        Duration::from_millis(400),
    )
    .await;
    match outcome {
        RunOutcome::Killed { message, init } => {
            assert!(message.contains("wall clock"), "{message}");
            assert!(init.is_some(), "init arrived before the hang");
        }
        other => panic!("expected killed, got {other:?}"),
    }
    assert_eq!(lines.len(), 2);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        marker_path.exists(),
        "the fake saw SIGTERM, not only SIGKILL"
    );
}

#[tokio::test]
async fn run_nonzero_exit_without_result() {
    let t = fixture("no-result.jsonl");
    let (outcome, _) = run_with(
        &[
            ("DAILYBRIEF_FAKE_TRANSCRIPT", t.to_str().unwrap()),
            ("DAILYBRIEF_FAKE_EXIT", "2"),
            ("DAILYBRIEF_FAKE_STDERR_BYTES", "12"),
        ],
        Duration::from_secs(10),
    )
    .await;
    match outcome {
        RunOutcome::Failed {
            reason,
            message,
            exit_code,
            ..
        } => {
            assert_eq!(reason, FailReason::Exit);
            assert_eq!(exit_code, Some(2));
            assert_eq!(message, "exit code 2: eeeeeeeeeeee");
        }
        other => panic!("expected failed, got {other:?}"),
    }
}

#[tokio::test]
async fn run_ends_without_result_line() {
    let t = fixture("no-result.jsonl");
    let (outcome, _) = run_with(
        &[("DAILYBRIEF_FAKE_TRANSCRIPT", t.to_str().unwrap())],
        Duration::from_secs(10),
    )
    .await;
    match outcome {
        RunOutcome::Failed {
            reason, exit_code, ..
        } => {
            assert_eq!(reason, FailReason::NoResult);
            assert_eq!(exit_code, Some(0));
        }
        other => panic!("expected failed, got {other:?}"),
    }
}

#[tokio::test]
async fn run_survives_chatty_stderr() {
    let t = fixture("success.jsonl");
    let (outcome, _) = run_with(
        &[
            ("DAILYBRIEF_FAKE_TRANSCRIPT", t.to_str().unwrap()),
            ("DAILYBRIEF_FAKE_STDERR_BYTES", "1048576"),
        ],
        Duration::from_secs(10),
    )
    .await;
    assert!(
        matches!(outcome, RunOutcome::Success { .. }),
        "1 MiB on stderr must not block the child: {outcome:?}"
    );
}

#[tokio::test]
async fn run_delivers_every_stdout_line_in_order_and_scrubs_env() {
    let t = fixture("success.jsonl");
    let (_, lines) = run_with(
        &[("DAILYBRIEF_FAKE_TRANSCRIPT", t.to_str().unwrap())],
        Duration::from_secs(10),
    )
    .await;
    let expected: Vec<String> = std::fs::read_to_string(&t)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        lines.iter().map(|(_, l)| l.clone()).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        lines.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        (1..=6).collect::<Vec<u64>>()
    );
    let HarnessKind::ClaudeCode(a) = adapter(&[], Duration::from_secs(1));
    assert!(!a.env.contains_key("SHOULD_NOT_LEAK"));
    assert!(a.env.contains_key("PATH"));
}

#[tokio::test]
async fn run_reports_a_missing_binary() {
    let settings = load_config(&Env::from_lookup(|_| None).unwrap())
        .unwrap()
        .harness
        .claude_code;
    let mut parent = HashMap::new();
    parent.insert("CLAUDE_CODE_OAUTH_TOKEN".to_string(), "t".to_string());
    let mut a = ClaudeCodeAdapter::new(settings, &parent).unwrap();
    a.binary = PathBuf::from("/nonexistent/claude");
    let dir = tempfile::tempdir().unwrap();
    let outcome = HarnessKind::ClaudeCode(a)
        .run(&request(dir.path()), |_, _| {})
        .await;
    match outcome {
        RunOutcome::Failed {
            reason, message, ..
        } => {
            assert_eq!(reason, FailReason::Exit);
            assert!(
                message.starts_with("could not start /nonexistent/claude"),
                "{message}"
            );
        }
        other => panic!("{other:?}"),
    }
}
