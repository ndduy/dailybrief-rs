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
        settings_path: cwd.join("settings.json"),
        system_prompt_path: root().join("prompts/editor.md"),
        user_message: USER_MESSAGE.into(),
        json_schema: serde_json::json!({ "type": "object" }),
    }
}

/// An adapter pointed at the fake, with the given DAILYBRIEF_FAKE_* knobs and a short wall clock.
fn adapter(knobs: &[(&str, &str)], wall_clock: Duration) -> HarnessKind {
    adapter_with_grace(knobs, wall_clock, Duration::from_secs(2))
}

fn adapter_with_grace(
    knobs: &[(&str, &str)],
    wall_clock: Duration,
    grace: Duration,
) -> HarnessKind {
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
    a.kill_grace = grace;
    HarnessKind::ClaudeCode(a)
}

async fn run_with(
    knobs: &[(&str, &str)],
    wall_clock: Duration,
) -> (RunOutcome, Vec<(u64, String)>) {
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(u64, String)>(1024);
    let collector = tokio::spawn(async move {
        let mut lines = Vec::new();
        while let Some(line) = rx.recv().await {
            lines.push(line);
        }
        lines
    });
    let outcome = adapter(knobs, wall_clock)
        .run(&request(dir.path()), tx)
        .await;
    let lines = collector.await.unwrap();
    (outcome, lines)
}

/// The child writes 10 000 lines faster than a deliberately slow consumer takes them: the
/// bounded channel makes the reader wait, nothing is dropped, and the order holds.
#[tokio::test]
async fn lines_are_never_dropped_under_backpressure() {
    let dir = tempfile::tempdir().unwrap();
    let long = dir.path().join("long.jsonl");
    let mut text = String::new();
    for i in 1..=10_000u32 {
        text.push_str(&format!(
            "{{\"type\":\"user\",\"message\":{{\"content\":\"line {i}\"}}}}\n"
        ));
    }
    text.push_str(&std::fs::read_to_string(fixture("success.jsonl")).unwrap());
    std::fs::write(&long, &text).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(u64, String)>(64);
    let collector = tokio::spawn(async move {
        let mut lines = Vec::new();
        while let Some(line) = rx.recv().await {
            if lines.len() % 50 == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            lines.push(line);
        }
        lines
    });
    let started = std::time::Instant::now();
    let outcome = adapter(
        &[("DAILYBRIEF_FAKE_TRANSCRIPT", long.to_str().unwrap())],
        Duration::from_secs(30),
    )
    .run(&request(dir.path()), tx)
    .await;
    let lines = collector.await.unwrap();
    assert!(matches!(outcome, RunOutcome::Success { .. }), "{outcome:?}");
    let expected = text.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(lines.len(), expected);
    for (i, (seq, raw)) in lines.iter().take(10_000).enumerate() {
        assert_eq!(*seq, i as u64 + 1);
        assert!(raw.ends_with(&format!("\"line {}\"}}}}", i + 1)), "{raw}");
    }
    assert!(started.elapsed() < Duration::from_secs(15));
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
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let outcome = HarnessKind::ClaudeCode(a)
        .run(&request(dir.path()), tx)
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

/// A child that closes stdout and stderr and then hangs must still die at the wall clock: the
/// deadline has to cover `wait()` after EOF, not only the read loop. (Found at the R0 ship
/// review; before the fix this test hung forever and the scheduler with it.)
#[tokio::test]
async fn run_hang_after_stdout_close_is_still_killed() {
    let t = fixture("no-result.jsonl");
    let marker = tempfile::tempdir().unwrap();
    let marker_path = marker.path().join("term");
    let started = std::time::Instant::now();
    let (outcome, lines) = tokio::time::timeout(
        Duration::from_secs(6),
        run_with(
            &[
                ("DAILYBRIEF_FAKE_TRANSCRIPT", t.to_str().unwrap()),
                ("DAILYBRIEF_FAKE_HANG", "1"),
                ("DAILYBRIEF_FAKE_CLOSE_STDOUT", "1"),
                ("DAILYBRIEF_FAKE_TERM_MARKER", marker_path.to_str().unwrap()),
            ],
            Duration::from_millis(400),
        ),
    )
    .await
    .expect("the adapter returned before the safety timeout");
    match outcome {
        RunOutcome::Killed { message, init } => {
            assert!(message.contains("wall clock"), "{message}");
            assert!(init.is_some());
        }
        other => panic!("expected killed, got {other:?}"),
    }
    assert_eq!(lines.len(), 2, "the lines before the close were delivered");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(marker_path.exists(), "SIGTERM was sent first");
}

/// Anything that looks like a token on the child's stderr is redacted before it can reach a
/// `runs.error` row or a page: the literal OAuth value and any `sk-ant-…` run.
#[tokio::test]
async fn stderr_tail_is_redacted_before_storage() {
    let t = fixture("no-result.jsonl");
    let (outcome, _) = run_with(
        &[
            ("DAILYBRIEF_FAKE_TRANSCRIPT", t.to_str().unwrap()),
            ("DAILYBRIEF_FAKE_EXIT", "1"),
            (
                "DAILYBRIEF_FAKE_STDERR_TEXT",
                "auth failed: token sk-ant-oat01-ABCdef_123-xyz rejected; env had fake-token",
            ),
        ],
        Duration::from_secs(10),
    )
    .await;
    let RunOutcome::Failed {
        message, reason, ..
    } = outcome
    else {
        panic!("{outcome:?}")
    };
    assert_eq!(reason, FailReason::Exit);
    assert!(message.contains("auth failed"), "{message}");
    assert!(!message.contains("sk-ant-oat01"), "{message}");
    assert!(!message.contains("fake-token"), "{message}");
    assert_eq!(message.matches("[redacted]").count(), 2, "{message}");
}

/// An invalid UTF-8 byte inside a line is replaced, not fatal: the run still reaches its result.
#[tokio::test]
async fn invalid_utf8_line_does_not_end_the_run() {
    let t = fixture("success.jsonl");
    let (outcome, lines) = run_with(
        &[
            ("DAILYBRIEF_FAKE_TRANSCRIPT", t.to_str().unwrap()),
            ("DAILYBRIEF_FAKE_BAD_UTF8", "1"),
        ],
        Duration::from_secs(10),
    )
    .await;
    assert!(matches!(outcome, RunOutcome::Success { .. }), "{outcome:?}");
    let expected = std::fs::read_to_string(&t)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    assert_eq!(lines.len(), expected + 1);
    assert!(lines[0].1.contains('\u{FFFD}'), "{}", lines[0].1);
}

/// `scan-transcript` on a run's transcript also inspects that run's stored error text.
#[tokio::test]
async fn scan_transcript_checks_runs_error() {
    use dailybrief::config::Env;
    use dailybrief::db::Db;
    use dailybrief::db::repo::{self, NewRun, Role, RunFinish, RunKind, RunStatus};
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let run_dir = data.join("runs").join("2026-09-17-leak");
    std::fs::create_dir_all(&run_dir).unwrap();
    let transcript = run_dir.join("transcript.jsonl");
    std::fs::copy(fixture("no-result.jsonl"), &transcript).unwrap();
    let db = Db::open(&data.join("brief.db")).unwrap();
    db.with(|c| {
        repo::insert_run(
            c,
            &NewRun {
                id: "2026-09-17-leak".into(),
                kind: RunKind::Manual,
                role: Role::Editor,
                harness: "claude-code".into(),
                attempt: 1,
                started_at: "2026-09-16T23:30:00.000Z".into(),
                transcript_path: Some(transcript.to_string_lossy().into_owned()),
            },
        )?;
        repo::finish_run(
            c,
            "2026-09-17-leak",
            &RunFinish {
                status: RunStatus::Failed,
                ended_at: "2026-09-16T23:31:00.000Z".into(),
                turns: None,
                usage_json: None,
                cost_usd: None,
                session_id: None,
                error: Some("exit code 1: OAUTH_TOKEN=sk-ant-oat01-leaked".into()),
            },
        )
    })
    .unwrap();
    drop(db);
    let data_s = data.to_string_lossy().into_owned();
    let env = Env::from_lookup(|n| (n == "DAILYBRIEF_DATA_DIR").then(|| data_s.clone())).unwrap();
    let mut out = Vec::new();
    let code = dailybrief::commands::scan_transcript::run(&env, Some(&transcript), None, &mut out)
        .await
        .unwrap();
    let text = String::from_utf8(out).unwrap();
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("runs.error"), "{text}");
    assert!(text.contains("sk-ant-"), "{text}");
}

/// A child that ignores SIGTERM is SIGKILLed once the grace period passes, and is gone.
#[tokio::test]
async fn run_hang_ignoring_sigterm_is_sigkilled_after_grace() {
    let t = fixture("no-result.jsonl");
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(u64, String)>(1024);
    let collector = tokio::spawn(async move {
        let mut n = 0;
        while rx.recv().await.is_some() {
            n += 1;
        }
        n
    });
    let started = std::time::Instant::now();
    let outcome = adapter_with_grace(
        &[
            ("DAILYBRIEF_FAKE_TRANSCRIPT", t.to_str().unwrap()),
            ("DAILYBRIEF_FAKE_HANG", "1"),
            ("DAILYBRIEF_FAKE_IGNORE_TERM", "1"),
            ("DAILYBRIEF_FAKE_PID_FILE", pid_file.to_str().unwrap()),
        ],
        Duration::from_millis(300),
        Duration::from_millis(500),
    )
    .run(&request(dir.path()), tx)
    .await;
    let elapsed = started.elapsed();
    assert!(matches!(outcome, RunOutcome::Killed { .. }), "{outcome:?}");
    assert!(
        elapsed >= Duration::from_millis(800),
        "wall + grace: {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert_eq!(collector.await.unwrap(), 2);
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .unwrap()
        .success();
    assert!(!alive, "pid {pid} still alive after SIGKILL");
}

/// A token in a stdout line or in the result's error text is redacted before storage, not only
/// on stderr: the delivered lines and the outcome message both read `[redacted]`.
#[tokio::test]
async fn stdout_lines_and_result_message_are_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let planted = dir.path().join("planted.jsonl");
    let mut text = std::fs::read_to_string(fixture("max-turns.jsonl")).unwrap();
    text = text.replace(
        "Reached maximum number of turns (120)",
        "auth header was Bearer fake-token then sk-ant-oat01-ZZZ9 rejected",
    );
    text.insert_str(
        0,
        "{\"type\":\"user\",\"message\":{\"content\":\"echo fake-token and sk-ant-oat01-ABC\"}}\n",
    );
    std::fs::write(&planted, &text).unwrap();
    let (outcome, lines) = run_with(
        &[("DAILYBRIEF_FAKE_TRANSCRIPT", planted.to_str().unwrap())],
        Duration::from_secs(10),
    )
    .await;
    let RunOutcome::Failed {
        message, reason, ..
    } = outcome
    else {
        panic!("{outcome:?}")
    };
    assert_eq!(reason, FailReason::MaxTurns);
    assert!(
        !message.contains("fake-token") && !message.contains("sk-ant-oat01"),
        "{message}"
    );
    assert!(message.contains("[redacted]"), "{message}");
    let joined: String = lines
        .iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!joined.contains("fake-token") && !joined.contains("sk-ant-oat01"));
    assert!(
        lines[0].1.contains("echo [redacted] and [redacted]"),
        "{}",
        lines[0].1
    );
}

/// If nobody is storing the lines (the writer died), a harness "success" is still a failed run.
#[tokio::test]
async fn dead_sink_makes_the_run_fail() {
    let t = fixture("success.jsonl");
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel::<(u64, String)>(8);
    drop(rx);
    let outcome = adapter(
        &[("DAILYBRIEF_FAKE_TRANSCRIPT", t.to_str().unwrap())],
        Duration::from_secs(10),
    )
    .run(&request(dir.path()), tx)
    .await;
    match outcome {
        RunOutcome::Failed {
            reason, message, ..
        } => {
            assert_eq!(reason, FailReason::Exit);
            assert!(message.contains("transcript writer stopped"), "{message}");
        }
        other => panic!("expected failed, got {other:?}"),
    }
}

/// A runaway stdout line (here 6 MiB) is stored cut at the cap with a marker, the rest of the
/// transcript still arrives, and the run still classifies (M3 code-review backlog #10).
#[tokio::test]
async fn stdout_line_size_is_bounded() {
    use dailybrief::harness::claude_code::{MAX_LINE_BYTES, TRUNCATED_MARKER};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("huge.jsonl");
    let mut text = String::with_capacity(6 * 1024 * 1024 + 512);
    text.push_str(r#"{"type":"assistant","message":{"content":[{"type":"text","text":""#);
    text.push_str(&"y".repeat(6 * 1024 * 1024));
    text.push_str("\"}]}}\n");
    text.push_str(
        std::fs::read_to_string(fixture("success.jsonl"))
            .unwrap()
            .trim_start(),
    );
    std::fs::write(&path, text).unwrap();
    let (outcome, lines) = run_with(
        &[("DAILYBRIEF_FAKE_TRANSCRIPT", path.to_str().unwrap())],
        Duration::from_secs(30),
    )
    .await;
    assert!(matches!(outcome, RunOutcome::Success { .. }), "{outcome:?}");
    assert_eq!(lines[0].0, 1);
    assert!(
        lines[0].1.len() <= MAX_LINE_BYTES + TRUNCATED_MARKER.len(),
        "{}",
        lines[0].1.len()
    );
    assert!(lines[0].1.ends_with(TRUNCATED_MARKER));
    assert_eq!(
        dailybrief::harness::trajectory::event_type(&lines[0].1),
        "unparseable"
    );
    let result = lines
        .iter()
        .filter(|(_, l)| l.contains("\"type\":\"result\""))
        .count();
    assert_eq!(result, 1, "the lines after the runaway one still arrive");
}
