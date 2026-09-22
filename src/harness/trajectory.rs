//! The typed view over `claude -p --output-format stream-json` lines (ADR 0012). Every line
//! the runner stored in `run_events` parses to one [`Event`]; shapes this module does not know
//! are kept as [`Event::Unknown`] with their raw value, never dropped. The vocabulary was
//! observed on Claude Code 2.1.274 (`spec/m2.md` §8). Pure: no I/O, no SQL.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

pub use super::claude_code::{InitEvent, McpServerStatus, ResultEvent};
use crate::config::{Caps, ClaudeCodeSettings};
use crate::db::repo::{RunEvent, RunRow, RunStatus};

/// One stdout line.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    System(SystemEvent),
    Assistant(MessageEvent),
    User(MessageEvent),
    ToolProgress(Value),
    RateLimitEvent(Value),
    Result(ResultEvent),
    /// A `type` this module does not know, a known type whose shape did not parse, or a line
    /// that is not JSON (`kind == "unparseable"`, `raw` is the text). Kept, shown, ignored by
    /// the folds.
    #[serde(skip)]
    Unknown {
        kind: String,
        raw: Value,
    },
}

/// `{"type":"system","subtype":"init"|"thinking_tokens"|…}`. The fields of every subtype seen
/// so far are optional here; a new subtype keeps its name and whatever fields it shares.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SystemEvent {
    #[serde(default)]
    pub subtype: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerStatus>,
    /// `thinking_tokens`: the running estimate.
    #[serde(default)]
    pub estimated_tokens: Option<u64>,
    #[serde(default)]
    pub estimated_tokens_delta: Option<u64>,
}

impl SystemEvent {
    /// The `init` line as the runner's `InitEvent`, when this is one.
    pub fn init(&self) -> Option<InitEvent> {
        (self.subtype == "init").then(|| InitEvent {
            subtype: self.subtype.clone(),
            session_id: self.session_id.clone(),
            model: self.model.clone(),
            tools: self.tools.clone(),
            mcp_servers: self.mcp_servers.clone(),
        })
    }
}

/// `assistant` and `user` lines: one API message plus the harness's envelope.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MessageEvent {
    #[serde(default)]
    pub message: Message,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

/// The API message. An assistant message arrives as several lines sharing one `id` (its
/// `text`, `thinking` and `tool_use` blocks); a user message carries the tool results.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Content,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

/// `content` is a string for plain user text and a block list otherwise.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Blocks(Vec<Block>),
}

impl Default for Content {
    fn default() -> Self {
        Self::Blocks(Vec::new())
    }
}

impl Content {
    pub fn blocks(&self) -> &[Block] {
        match self {
            Self::Blocks(b) => b,
            Self::Text(_) => &[],
        }
    }
}

/// One content block; unknown block types collapse to `Other`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        #[serde(default)]
        text: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    ToolUse {
        #[serde(default)]
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        input: Value,
    },
    ToolResult {
        #[serde(default)]
        tool_use_id: String,
        #[serde(default)]
        content: Value,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(other)]
    Other,
}

/// Token accounting as the API reports it on each message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
}

impl Event {
    /// Parses an already-decoded line. A missing `type` or a shape this module cannot parse is
    /// `Unknown` with the whole value kept.
    pub fn from_value(value: Value) -> Event {
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            return Event::Unknown {
                kind: "unparseable".to_string(),
                raw: value,
            };
        };
        let kind = kind.to_string();
        match serde_json::from_value::<Event>(value.clone()) {
            Ok(event) => event,
            Err(_) => Event::Unknown { kind, raw: value },
        }
    }

    /// The `run_events.type` string for this event, exactly as R0 stored it.
    pub fn kind(&self) -> &str {
        match self {
            Event::System(_) => "system",
            Event::Assistant(_) => "assistant",
            Event::User(_) => "user",
            Event::ToolProgress(_) => "tool_progress",
            Event::RateLimitEvent(_) => "rate_limit_event",
            Event::Result(_) => "result",
            Event::Unknown { kind, .. } => kind,
        }
    }
}

/// Parses one raw stdout line; never fails.
pub fn parse_line(raw: &str) -> Event {
    match serde_json::from_str::<Value>(raw) {
        Ok(value) => Event::from_value(value),
        Err(_) => Event::Unknown {
            kind: "unparseable".to_string(),
            raw: Value::String(raw.to_string()),
        },
    }
}

/// The `run_events.type` value for a raw line: its `type` field, or `unparseable`.
pub fn event_type(raw: &str) -> String {
    parse_line(raw).kind().to_string()
}

// ---------- folding: turns, caps, retry chain (ADR 0012) ----------

const ARGS_CHARS: usize = 120;
const TEXT_CHARS: usize = 120;

fn truncate(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().nth(max).is_some() {
        out.push('…');
    }
    out
}

/// `mcp__dailybrief__read_item` → `read_item`; built-ins keep their name.
pub fn short_tool(name: &str) -> &str {
    name.strip_prefix("mcp__dailybrief__").unwrap_or(name)
}

fn display_tool(tools: &[String]) -> Option<String> {
    let shorts: Vec<&str> = tools.iter().map(|t| short_tool(t)).collect();
    match shorts.as_slice() {
        [] => None,
        [one] => Some((*one).to_string()),
        [first, rest @ ..] if rest.iter().all(|s| s == first) => {
            Some(format!("{first} ×{}", shorts.len()))
        }
        _ => Some(shorts.join(" + ")),
    }
}

/// Characters of text in a `tool_result` content (a string, or `{type: text}` parts).
fn result_len(content: &Value) -> usize {
    match content {
        Value::String(s) => s.chars().count(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .map(|t| t.chars().count())
            .sum(),
        Value::Null => 0,
        other => other.to_string().chars().count(),
    }
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Token accounting for one turn (the last line of the message wins).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_create: u64,
}

/// One assistant message: what it called, what came back, when, and what it cost.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Turn {
    /// 1-based.
    pub n: u32,
    pub message_id: Option<String>,
    /// The first line's `timestamp`.
    pub at: Option<String>,
    /// Display name: `read_item`, `read_item ×2`, `search_items + select`; `None` without a tool.
    pub tool: Option<String>,
    /// Every tool called in this turn, full names, in order.
    pub tools: Vec<String>,
    /// Compact JSON of the first tool call's input, capped.
    pub args_summary: String,
    /// Characters of text returned across this turn's tool results.
    pub result_chars: usize,
    /// Tool results the server marked `is_error` (a rejected select, an unknown item).
    pub errors: u32,
    /// Full names of the tools whose result was `is_error`, in order.
    pub failed_tools: Vec<String>,
    /// Seconds since the previous turn's timestamp; `None` for the first turn or without timestamps.
    pub since_prev_secs: Option<f64>,
    pub tokens: Tokens,
    /// The first text block, capped.
    pub text_preview: String,
}

/// Groups the stored events into turns: one per assistant `message.id` (consecutive lines that
/// share it are one turn; a line without an id is its own turn), with each `tool_result`
/// credited to the turn that issued the matching `tool_use`. Unknown lines and orphan results
/// are ignored, never a panic.
pub fn fold_turns(events: &[RunEvent]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut by_tool_use: HashMap<String, (usize, String)> = HashMap::new();
    let mut open: Option<Option<String>> = None;
    for e in events {
        match parse_line(&e.payload_json) {
            Event::Assistant(m) => {
                let id = m.message.id.clone();
                let same = matches!((&open, &id), (Some(Some(o)), Some(i)) if o == i);
                if !same {
                    turns.push(Turn {
                        n: turns.len() as u32 + 1,
                        message_id: id.clone(),
                        at: m.timestamp.clone(),
                        ..Turn::default()
                    });
                    open = Some(id);
                }
                let idx = turns.len() - 1;
                if let Some(u) = m.message.usage {
                    turns[idx].tokens = Tokens {
                        input: u.input_tokens,
                        output: u.output_tokens,
                        cache_read: u.cache_read_input_tokens,
                        cache_create: u.cache_creation_input_tokens,
                    };
                }
                for b in m.message.content.blocks() {
                    match b {
                        Block::ToolUse { id, name, input } => {
                            turns[idx].tools.push(name.clone());
                            by_tool_use.insert(id.clone(), (idx, name.clone()));
                            if turns[idx].args_summary.is_empty() {
                                turns[idx].args_summary = truncate(&input.to_string(), ARGS_CHARS);
                            }
                        }
                        Block::Text { text } if turns[idx].text_preview.is_empty() => {
                            turns[idx].text_preview = truncate(text.trim(), TEXT_CHARS);
                        }
                        _ => {}
                    }
                }
                turns[idx].tool = display_tool(&turns[idx].tools);
            }
            Event::User(m) => {
                for b in m.message.content.blocks() {
                    if let Block::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = b
                        && let Some((idx, name)) = by_tool_use.get(tool_use_id)
                    {
                        let idx = *idx;
                        turns[idx].result_chars += result_len(content);
                        if *is_error {
                            turns[idx].errors += 1;
                            turns[idx].failed_tools.push(name.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let mut prev: Option<DateTime<Utc>> = None;
    for t in &mut turns {
        let at = t.at.as_deref().and_then(parse_ts);
        t.since_prev_secs = match (prev, at) {
            (Some(p), Some(a)) => Some((a - p).num_milliseconds() as f64 / 1000.0),
            _ => None,
        };
        if at.is_some() {
            prev = at;
        }
    }
    turns
}

/// One counter against its cap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Used {
    pub used: u64,
    pub cap: u64,
    pub hit: bool,
}

/// What a run consumed, against the configured caps (`spec/m2.md` §8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapsUsed {
    pub reads: Used,
    /// Accepted `select` calls (a rejected one is not a selection) against
    /// `for_you + beyond_radar`.
    pub selects: Used,
    /// `WebSearch` calls against `web_search`.
    pub searches: Used,
    /// Turns against `max_turns`; `hit` also when the result says `error_max_turns`.
    pub turns: Used,
    /// Wall clock in seconds (the result's `duration_ms`, else first-to-last timestamp) against
    /// `wall_clock_minutes`.
    pub wall_secs: Used,
    pub publishes: u64,
    pub max_turns_hit: bool,
}

pub fn caps_used(
    turns: &[Turn],
    result: Option<&ResultEvent>,
    caps: &Caps,
    settings: &ClaudeCodeSettings,
) -> CapsUsed {
    let count = |name: &str| {
        turns
            .iter()
            .flat_map(|t| t.tools.iter())
            .filter(|t| short_tool(t) == name)
            .count() as u64
    };
    let failed = |name: &str| {
        turns
            .iter()
            .flat_map(|t| t.failed_tools.iter())
            .filter(|t| short_tool(t) == name)
            .count() as u64
    };
    let mark = |used: u64, cap: u64| Used {
        used,
        cap,
        hit: cap > 0 && used >= cap,
    };
    let max_turns_hit = result.is_some_and(|r| r.subtype == "error_max_turns");
    let wall = match result.and_then(|r| r.duration_ms) {
        Some(ms) => (ms + 500) / 1000,
        None => {
            let stamps: Vec<DateTime<Utc>> = turns
                .iter()
                .filter_map(|t| t.at.as_deref().and_then(parse_ts))
                .collect();
            match (stamps.first(), stamps.last()) {
                (Some(a), Some(b)) => (*b - *a).num_seconds().max(0) as u64,
                _ => 0,
            }
        }
    };
    let turn_count = turns.len() as u64;
    // A run that ended in `error_max_turns` reached its cap by definition, so the bar reads
    // `turns N/N` at the turns we count (the harness's own `num_turns` is one higher, ADR 0012);
    // this is also right when the CLI overrode the configured value for that run.
    let max_turns = if max_turns_hit {
        turn_count
    } else {
        u64::from(settings.max_turns)
    };
    let wall_cap = u64::from(settings.wall_clock_minutes) * 60;
    CapsUsed {
        reads: mark(count("read_item"), u64::from(caps.reads)),
        selects: mark(
            count("select").saturating_sub(failed("select")),
            u64::from(caps.for_you) + u64::from(caps.beyond_radar),
        ),
        searches: mark(count("WebSearch"), u64::from(caps.web_search)),
        turns: Used {
            used: turn_count,
            cap: max_turns,
            hit: max_turns_hit || (max_turns > 0 && turn_count >= max_turns),
        },
        wall_secs: Used {
            used: wall,
            cap: wall_cap,
            hit: wall_cap > 0 && wall >= wall_cap,
        },
        publishes: count("publish_digest"),
        max_turns_hit,
    }
}

/// One attempt of a run summary.
#[derive(Debug, Clone, PartialEq)]
pub struct AttemptOutcome {
    pub run_id: String,
    pub attempt: i64,
    pub status: RunStatus,
    pub error: Option<String>,
    pub started_at: String,
}

fn secs_between(a: &str, b: &str) -> i64 {
    match (parse_ts(a), parse_ts(b)) {
        (Some(a), Some(b)) => (b - a).num_seconds().abs(),
        _ => i64::MAX,
    }
}

/// The attempts that belong with `target`: same kind, attempt numbers consecutive, each started
/// within `window_secs` of the previous (the runner starts attempt 2 right after attempt 1
/// ends, always inside the lock window). Oldest first; empty when `target` is not in `rows`.
pub fn retry_chain(target: &str, rows: &[RunRow], window_secs: i64) -> Vec<AttemptOutcome> {
    let mut sorted: Vec<&RunRow> = rows.iter().collect();
    sorted.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
    let Some(pos) = sorted.iter().position(|r| r.id == target) else {
        return Vec::new();
    };
    let close = |a: &RunRow, b: &RunRow| {
        a.kind == b.kind && secs_between(&a.started_at, &b.started_at) <= window_secs
    };
    let mut first = pos;
    while first > 0
        && sorted[first - 1].attempt == sorted[first].attempt - 1
        && sorted[first - 1].attempt >= 1
        && close(sorted[first - 1], sorted[first])
    {
        first -= 1;
    }
    let mut last = pos;
    while last + 1 < sorted.len()
        && sorted[last + 1].attempt == sorted[last].attempt + 1
        && close(sorted[last], sorted[last + 1])
    {
        last += 1;
    }
    sorted[first..=last]
        .iter()
        .map(|r| AttemptOutcome {
            run_id: r.id.clone(),
            attempt: r.attempt,
            status: r.status,
            error: r.error.clone(),
            started_at: r.started_at.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_content_may_be_a_plain_string() {
        match parse_line(
            r#"{"type":"user","message":{"role":"user","content":"Build today's digest."}}"#,
        ) {
            Event::User(m) => {
                assert!(matches!(&m.message.content, Content::Text(t) if t.starts_with("Build")));
                assert!(m.message.content.blocks().is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn tool_use_and_result_blocks_keep_ids_and_payloads() {
        let a = parse_line(
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"mcp__dailybrief__read_item","input":{"id":"abc"}}],"usage":{"input_tokens":2,"output_tokens":8,"cache_read_input_tokens":3489,"cache_creation_input_tokens":0}}}"#,
        );
        let Event::Assistant(m) = a else {
            panic!("{a:?}")
        };
        assert_eq!(m.message.usage.unwrap().cache_read_input_tokens, 3489);
        match &m.message.content.blocks()[0] {
            Block::ToolUse { id, name, input } => {
                assert_eq!(id, "t1");
                assert_eq!(name, "mcp__dailybrief__read_item");
                assert_eq!(input["id"], "abc");
            }
            other => panic!("{other:?}"),
        }
        let u = parse_line(
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"{\"ok\":true}"}]}}"#,
        );
        let Event::User(m) = u else { panic!("{u:?}") };
        assert!(matches!(
            &m.message.content.blocks()[0],
            Block::ToolResult { tool_use_id, is_error: false, .. } if tool_use_id == "t1"
        ));
    }

    #[test]
    fn a_known_type_with_an_unparseable_shape_is_unknown_with_that_kind() {
        // `message` must be an object; a string does not parse, so the line is kept as unknown.
        match parse_line(r#"{"type":"assistant","message":"nope"}"#) {
            Event::Unknown { kind, raw } => {
                assert_eq!(kind, "assistant");
                assert_eq!(raw["message"], "nope");
            }
            other => panic!("{other:?}"),
        }
    }
}
