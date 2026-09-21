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
