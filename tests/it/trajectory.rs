//! The typed view over `stream-json` (ADR 0012): every line of the first real run parses to a
//! known variant, unknown shapes are kept, and the stored `run_events.type` strings are exactly
//! what R0 wrote.

use std::path::PathBuf;

use dailybrief::harness::trajectory::{Block, Content, Event, event_type, parse_line};

fn fixture_lines() -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/transcripts/real-2026-09-21-c8354589.jsonl");
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

#[test]
fn every_fixture_line_parses_to_a_known_variant() {
    let lines = fixture_lines();
    assert_eq!(lines.len(), 337);
    let (mut system, mut assistant, mut user, mut progress, mut rate, mut result, mut unknown) =
        (0, 0, 0, 0, 0, 0, 0);
    let mut init_seen = false;
    let mut tool_uses = 0;
    let mut tool_results = 0;
    for raw in &lines {
        match parse_line(raw) {
            Event::System(s) => {
                system += 1;
                if let Some(init) = s.init() {
                    init_seen = true;
                    assert_eq!(init.mcp_servers[0].name, "dailybrief");
                    assert_eq!(init.tools.len(), 11);
                } else {
                    assert_eq!(s.subtype, "thinking_tokens");
                    assert!(s.estimated_tokens.is_some());
                }
            }
            Event::Assistant(m) => {
                assistant += 1;
                assert!(m.message.id.is_some());
                assert!(m.timestamp.is_some());
                if let Content::Blocks(blocks) = &m.message.content {
                    tool_uses += blocks
                        .iter()
                        .filter(|b| matches!(b, Block::ToolUse { .. }))
                        .count();
                }
            }
            Event::User(m) => {
                user += 1;
                if let Content::Blocks(blocks) = &m.message.content {
                    tool_results += blocks
                        .iter()
                        .filter(|b| matches!(b, Block::ToolResult { .. }))
                        .count();
                }
            }
            Event::ToolProgress(_) => progress += 1,
            Event::RateLimitEvent(_) => rate += 1,
            Event::Result(r) => {
                result += 1;
                assert_eq!(r.subtype, "success");
                assert_eq!(r.num_turns, Some(78));
            }
            Event::Unknown { .. } => unknown += 1,
        }
    }
    assert_eq!(
        (system, assistant, user, progress, rate, result, unknown),
        (148, 107, 77, 1, 3, 1, 0)
    );
    assert!(init_seen);
    assert_eq!(
        tool_uses, 77,
        "one tool_use per assistant message that called a tool"
    );
    assert_eq!(tool_results, 77);
}

#[test]
fn unknown_type_and_unknown_block_are_kept_not_dropped() {
    match parse_line(r#"{"type":"future_thing","x":1}"#) {
        Event::Unknown { kind, raw } => {
            assert_eq!(kind, "future_thing");
            assert_eq!(raw["x"], 1);
        }
        other => panic!("{other:?}"),
    }
    let line = r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"hologram","z":2},{"type":"text","text":"hi"}]}}"#;
    match parse_line(line) {
        Event::Assistant(m) => match m.message.content {
            Content::Blocks(blocks) => {
                assert!(matches!(blocks[0], Block::Other));
                assert!(matches!(&blocks[1], Block::Text { text } if text == "hi"));
            }
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn new_system_subtype_keeps_its_name_and_invalid_json_is_unknown() {
    match parse_line(r#"{"type":"system","subtype":"compaction","session_id":"s"}"#) {
        Event::System(s) => {
            assert_eq!(s.subtype, "compaction");
            assert!(s.init().is_none());
        }
        other => panic!("{other:?}"),
    }
    match parse_line("not json {") {
        Event::Unknown { kind, raw } => {
            assert_eq!(kind, "unparseable");
            assert_eq!(raw, serde_json::Value::String("not json {".into()));
        }
        other => panic!("{other:?}"),
    }
    match parse_line(r#"{"no_type":true}"#) {
        Event::Unknown { kind, .. } => assert_eq!(kind, "unparseable"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn event_type_strings_are_unchanged() {
    for (line, expected) in [
        (r#"{"type":"system","subtype":"init"}"#, "system"),
        (r#"{"type":"system","subtype":"thinking_tokens"}"#, "system"),
        (
            r#"{"type":"assistant","message":{"content":[]}}"#,
            "assistant",
        ),
        (r#"{"type":"user","message":{"content":"hi"}}"#, "user"),
        (r#"{"type":"result","subtype":"success"}"#, "result"),
        (r#"{"type":"rate_limit_event"}"#, "rate_limit_event"),
        (r#"{"type":"tool_progress"}"#, "tool_progress"),
        (r#"{"type":"something_new"}"#, "something_new"),
        ("garbage", "unparseable"),
    ] {
        assert_eq!(event_type(line), expected, "{line}");
    }
}

// ---------- Task 2: folding ----------

use dailybrief::config::{Caps, ClaudeCodeSettings};
use dailybrief::db::repo::{RunEvent, RunRow, RunStatus};
use dailybrief::harness::trajectory::{caps_used, fold_turns, retry_chain};

fn events(lines: &[String]) -> Vec<RunEvent> {
    lines
        .iter()
        .enumerate()
        .map(|(i, raw)| RunEvent {
            seq: i as i64 + 1,
            kind: event_type(raw),
            payload_json: raw.clone(),
        })
        .collect()
}

fn caps() -> Caps {
    Caps {
        reads: 45,
        for_you: 24,
        beyond_radar: 6,
        per_source: 4,
        per_topic: 8,
        web_search: 5,
        read_text_chars: 5000,
        shown_days: 14,
        candidate_days: 7,
    }
}

fn settings() -> ClaudeCodeSettings {
    ClaudeCodeSettings {
        model: "opus".into(),
        max_turns: 120,
        wall_clock_minutes: 15,
    }
}

fn result_of(lines: &[String]) -> dailybrief::harness::trajectory::ResultEvent {
    lines
        .iter()
        .find_map(|l| match parse_line(l) {
            Event::Result(r) => Some(r),
            _ => None,
        })
        .unwrap()
}

/// ADR 0012: a turn is one assistant message id; `num_turns` counts one more (the final round
/// trip). Tokens per turn are what each message reports and are not reconciled with the result.
#[test]
fn fixture_turns_reconcile_with_the_result_line() {
    let lines = fixture_lines();
    let evs = events(&lines);
    let turns = fold_turns(&evs);
    let result = result_of(&lines);
    let num_turns = result.num_turns.unwrap() as usize;
    assert!(
        (num_turns - 1..=num_turns).contains(&turns.len()),
        "turns {} vs num_turns {num_turns}",
        turns.len()
    );
    assert_eq!(turns.len(), 77);
    let used = caps_used(&turns, Some(&result), &caps(), &settings());
    assert_eq!(
        (used.reads.used, used.reads.cap, used.reads.hit),
        (32, 45, false)
    );
    assert_eq!(
        (used.selects.used, used.selects.cap, used.selects.hit),
        (31, 30, true)
    );
    assert_eq!((used.searches.used, used.searches.cap), (0, 5));
    assert_eq!(
        (used.turns.used, used.turns.cap, used.turns.hit),
        (77, 120, false)
    );
    assert!(
        (used.wall_secs.used as i64 - 500).abs() <= 1,
        "{}",
        used.wall_secs.used
    );
    assert_eq!(used.wall_secs.cap, 900);
    assert!(used.publishes == 1);
    // First turn: the briefing call; last turn: the structured output.
    assert_eq!(turns[0].tool.as_deref(), Some("get_briefing"));
    assert_eq!(turns[0].n, 1);
    assert!(turns[0].since_prev_secs.is_none());
    assert_eq!(turns[76].tool.as_deref(), Some("StructuredOutput"));
    // Every turn that called read_item got its text back.
    let reads: Vec<_> = turns
        .iter()
        .filter(|t| t.tool.as_deref() == Some("read_item"))
        .collect();
    assert_eq!(reads.len(), 32);
    assert!(
        reads.iter().all(|t| t.result_chars > 400),
        "every read returned text"
    );
    assert_eq!(reads.iter().map(|t| t.result_chars).sum::<usize>(), 135_443);
    assert!(turns.iter().skip(1).all(|t| t.since_prev_secs.is_some()));
    assert!(turns.iter().all(|t| t.tokens.output > 0));
}

#[test]
fn turns_from_hand_written_events() {
    let lines: Vec<String> = [
        r#"{"type":"system","subtype":"init","tools":[]}"#,
        r#"{"type":"assistant","timestamp":"2026-09-21T05:00:00.000Z","message":{"id":"m1","content":[{"type":"text","text":"Thinking about it."}],"usage":{"output_tokens":5}}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-21T05:00:02.000Z","message":{"id":"m2","content":[{"type":"tool_use","id":"t1","name":"mcp__dailybrief__read_item","input":{"id":"a"}},{"type":"tool_use","id":"t2","name":"mcp__dailybrief__read_item","input":{"id":"b"}}],"usage":{"output_tokens":9}}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"xxxxx"},{"type":"tool_result","tool_use_id":"t2","content":[{"type":"text","text":"yyy"}]}]}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-21T05:00:12.500Z","message":{"id":"m3","content":[{"type":"tool_use","id":"t3","name":"WebSearch","input":{"query":"rust"}}],"usage":{"output_tokens":3}}}"#,
        r#"{"type":"assistant","timestamp":"2026-09-21T05:00:12.600Z","message":{"id":"m3","content":[{"type":"text","text":"and"}],"usage":{"output_tokens":4}}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t3","content":"zz"}]}}"#,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let turns = fold_turns(&events(&lines));
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[0].tool, None);
    assert_eq!(turns[0].text_preview, "Thinking about it.");
    assert_eq!(turns[0].tokens.output, 5);
    assert_eq!(turns[1].tool.as_deref(), Some("read_item ×2"));
    assert_eq!(
        turns[1].tools,
        vec!["mcp__dailybrief__read_item", "mcp__dailybrief__read_item"]
    );
    assert_eq!(turns[1].result_chars, 8);
    assert_eq!(turns[1].since_prev_secs, Some(2.0));
    assert!(turns[1].args_summary.contains("\"id\":\"a\""));
    assert_eq!(turns[2].tool.as_deref(), Some("WebSearch"));
    assert_eq!(turns[2].tokens.output, 4, "the last line of a message wins");
    assert_eq!(turns[2].since_prev_secs, Some(10.5));
    assert_eq!(turns[2].result_chars, 2);
}

#[test]
fn orphan_tool_result_and_unknown_lines_do_not_panic() {
    let lines: Vec<String> = [
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"nope","content":"x"}]}}"#,
        "garbage",
        r#"{"type":"future","x":1}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"no id"}]}}"#,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let turns = fold_turns(&events(&lines));
    assert_eq!(
        turns.len(),
        1,
        "an assistant line without an id is still a turn"
    );
    assert_eq!(turns[0].result_chars, 0);
}

#[test]
fn caps_used_marks_the_hit_cap() {
    let lines: Vec<String> = (1..=5)
        .map(|i| format!(r#"{{"type":"assistant","message":{{"id":"m{i}","content":[{{"type":"tool_use","id":"t{i}","name":"mcp__dailybrief__read_item","input":{{}}}}]}}}}"#))
        .chain(std::iter::once(r#"{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":5,"duration_ms":30000}"#.to_string()))
        .collect();
    let evs = events(&lines);
    let turns = fold_turns(&evs);
    let result = result_of(&lines);
    let mut s = settings();
    s.max_turns = 5;
    let used = caps_used(&turns, Some(&result), &caps(), &s);
    assert_eq!(
        (used.turns.used, used.turns.cap, used.turns.hit),
        (5, 5, true)
    );
    assert_eq!(used.wall_secs.used, 30);
    assert!(!used.reads.hit);
    let none = caps_used(&turns, None, &caps(), &s);
    assert_eq!(
        none.wall_secs.used, 0,
        "no timestamps and no result: unknown wall"
    );
}

fn row(id: &str, kind: &str, attempt: i64, started: &str, status: RunStatus) -> RunRow {
    RunRow {
        id: id.into(),
        kind: kind.into(),
        harness: "claude-code".into(),
        status,
        attempt,
        started_at: started.into(),
        ended_at: None,
        turns: None,
        usage_json: None,
        cost_usd: None,
        session_id: None,
        error: (status == RunStatus::Failed).then(|| "boom".to_string()),
        transcript_path: None,
    }
}

#[test]
fn retry_chain_orders_attempts_and_ignores_unrelated_runs() {
    let newest_first = vec![
        row(
            "r4",
            "manual",
            1,
            "2026-09-21T09:00:00.000Z",
            RunStatus::Success,
        ),
        row(
            "r3",
            "scheduled",
            2,
            "2026-09-20T23:41:00.000Z",
            RunStatus::Success,
        ),
        row(
            "r2",
            "scheduled",
            1,
            "2026-09-20T23:30:00.000Z",
            RunStatus::Failed,
        ),
        row(
            "r1",
            "scheduled",
            1,
            "2026-09-19T23:30:00.000Z",
            RunStatus::Success,
        ),
    ];
    let window = 35 * 60;
    let chain = retry_chain("r3", &newest_first, window);
    assert_eq!(
        chain.iter().map(|a| a.run_id.as_str()).collect::<Vec<_>>(),
        ["r2", "r3"]
    );
    assert_eq!(chain[0].attempt, 1);
    assert_eq!(chain[0].status, RunStatus::Failed);
    assert_eq!(chain[0].error.as_deref(), Some("boom"));
    let same = retry_chain("r2", &newest_first, window);
    assert_eq!(
        same.iter().map(|a| a.run_id.as_str()).collect::<Vec<_>>(),
        ["r2", "r3"]
    );
    let alone = retry_chain("r1", &newest_first, window);
    assert_eq!(
        alone.iter().map(|a| a.run_id.as_str()).collect::<Vec<_>>(),
        ["r1"]
    );
    let manual = retry_chain("r4", &newest_first, window);
    assert_eq!(manual.len(), 1);
    assert!(retry_chain("missing", &newest_first, window).is_empty());
}
