//! Tool errors (`SPEC.md` §7a): one variant per contract violation, rendered to one imperative
//! sentence the agent can act on. Every variant becomes an `isError` tool result, never a
//! JSON-RPC error, so the harness sees feedback instead of a dead tool.

use rmcp::model::CallToolResult;
use serde_json::{Value, json};

use crate::core::digest::PublishError;
use crate::core::staging::StagingError;
use crate::db::DbError;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error(transparent)]
    Staging(#[from] StagingError),
    #[error("No feed with id '{0}'. Use the feedHealth list from get_briefing.")]
    UnknownFeed(String),
    #[error("Unknown issue kind '{0}'. Kinds: dead, paywalled, junk, duplicate.")]
    UnknownIssueKind(String),
    #[error("Notes are {0} characters; the cap is 2000.")]
    NotesTooLong(usize),
    #[error("editor_notes replace needs a text.")]
    NotesMissing,
    #[error("The tool failed internally; try once more, then move on.")]
    Internal(String),
}

impl From<DbError> for ToolError {
    fn from(e: DbError) -> Self {
        Self::Internal(e.to_string())
    }
}

impl ToolError {
    /// The wire form: `{"error": "<sentence>"}` with `isError: true`.
    pub fn into_result(self) -> CallToolResult {
        if let Self::Internal(detail) = &self {
            tracing::error!(detail, "tool failed internally");
        }
        CallToolResult::structured_error(json!({ "error": self.to_string() }))
    }
}

/// A successful tool value: compact JSON text plus the same object as `structuredContent`.
pub fn ok(value: Value) -> CallToolResult {
    CallToolResult::structured(value)
}

/// A structured rejection the agent must fix (publish violations): full object, `isError: true`.
pub fn rejected(value: Value) -> CallToolResult {
    CallToolResult::structured_error(value)
}

impl From<PublishError> for ToolError {
    fn from(e: PublishError) -> Self {
        match e {
            PublishError::Db(d) => Self::Internal(d.to_string()),
            other => Self::Internal(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_error_renders_one_sentence_with_is_error() {
        let r = ToolError::UnknownFeed("x".into()).into_result();
        assert_eq!(r.is_error, Some(true));
        let text = r.content[0].as_text().expect("text content").text.clone();
        assert_eq!(
            text,
            "{\"error\":\"No feed with id 'x'. Use the feedHealth list from get_briefing.\"}"
        );
        assert_eq!(
            r.structured_content.unwrap()["error"],
            "No feed with id 'x'. Use the feedHealth list from get_briefing."
        );
    }

    #[test]
    fn staging_errors_pass_through_verbatim() {
        let r = ToolError::from(StagingError::ReadCap { cap: 45 }).into_result();
        assert_eq!(
            r.structured_content.unwrap()["error"],
            "Read cap of 45 reached for this run. Select from what you have read."
        );
    }
}
