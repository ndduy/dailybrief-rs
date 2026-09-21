//! The typed view over `claude -p --output-format stream-json` lines (ADR 0012). Every line
//! the runner stored in `run_events` parses to one [`Event`]; shapes this module does not know
//! are kept as [`Event::Unknown`] with their raw value, never dropped. The vocabulary was
//! observed on Claude Code 2.1.274 (`spec/m2.md` §8). Pure: no I/O, no SQL.

use serde::Deserialize;
use serde_json::Value;

pub use super::claude_code::{InitEvent, McpServerStatus, ResultEvent};

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
