//! `editor_notes({ op, text? })` → `{ notes }`. One row, ≤ 2 000 characters: the Editor's memory
//! across runs and harnesses.

use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::time::to_iso;
use crate::db::repo;
use crate::mcp::error::{ToolError, ok};
use crate::mcp::server::DailyBriefServer;

pub const NOTES_MAX_CHARS: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotesOp {
    Read,
    Replace,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditorNotesInput {
    /// read | replace
    pub op: NotesOp,
    /// The full new text for replace (at most 2000 characters); ignored for read.
    #[serde(default)]
    pub text: Option<String>,
}

pub async fn run(
    s: &DailyBriefServer,
    input: EditorNotesInput,
) -> Result<CallToolResult, ToolError> {
    let notes = match input.op {
        NotesOp::Read => s.db.call(|conn| repo::get_editor_notes(conn)).await?,
        NotesOp::Replace => {
            let text = input.text.ok_or(ToolError::NotesMissing)?;
            let len = text.chars().count();
            if len > NOTES_MAX_CHARS {
                return Err(ToolError::NotesTooLong(len));
            }
            let at = to_iso((s.now)());
            let stored = text.clone();
            s.db.call(move |conn| repo::set_editor_notes(conn, &stored, &at))
                .await?;
            text
        }
    };
    Ok(ok(json!({ "notes": notes })))
}

#[cfg(test)]
mod tests {
    use crate::db::Db;
    use crate::mcp::server::testkit::*;
    use serde_json::json;

    #[tokio::test]
    async fn editor_notes_roundtrip() {
        let h = Harness::start(server_with(Db::open_in_memory().unwrap())).await;
        let empty = structured(&h.call("editor_notes", json!({ "op": "read" })).await);
        assert_eq!(empty, json!({ "notes": "" }));
        let set = structured(
            &h.call(
                "editor_notes",
                json!({ "op": "replace", "text": "prefer short posts" }),
            )
            .await,
        );
        assert_eq!(set["notes"], "prefer short posts");
        let read = structured(&h.call("editor_notes", json!({ "op": "read" })).await);
        assert_eq!(read["notes"], "prefer short posts");
    }

    #[tokio::test]
    async fn editor_notes_rejects_over_2000_chars_and_missing_text() {
        let h = Harness::start(server_with(Db::open_in_memory().unwrap())).await;
        let long = "ơ".repeat(2001);
        let r = h
            .call("editor_notes", json!({ "op": "replace", "text": long }))
            .await;
        assert_eq!(r.is_error, Some(true));
        assert_eq!(
            structured(&r)["error"],
            "Notes are 2001 characters; the cap is 2000."
        );
        let exact = "ơ".repeat(2000);
        let r = h
            .call("editor_notes", json!({ "op": "replace", "text": exact }))
            .await;
        assert_eq!(
            r.is_error,
            Some(false),
            "2000 chars (4000 bytes) is allowed"
        );
        let r = h.call("editor_notes", json!({ "op": "replace" })).await;
        assert_eq!(
            structured(&r)["error"],
            "editor_notes replace needs a text."
        );
    }
}
