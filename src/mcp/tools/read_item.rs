//! `read_item({ id })` → `{ id, title, source, text, wordCount, truncated }`; counts toward the
//! 45-read cap and records the read for read-before-select.

use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::core::staging::read_item;
use crate::mcp::error::{ToolError, ok};
use crate::mcp::server::DailyBriefServer;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadItemInput {
    /// Item id from list_candidates or search_items.
    pub id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadItemOutput {
    pub id: String,
    pub title: String,
    pub source: String,
    /// The first 5000 characters of the article.
    pub text: String,
    pub word_count: i64,
    pub truncated: bool,
}

pub async fn run(s: &DailyBriefServer, input: ReadItemInput) -> Result<CallToolResult, ToolError> {
    let caps = s.config.caps;
    let run_id = s.run_id.clone();
    let id = input.id.trim().to_string();
    let read =
        s.db.call(move |conn| Ok(read_item(conn, &caps, &run_id, &id)))
            .await??;
    let out = ReadItemOutput {
        id: read.id,
        title: read.title,
        source: read.source,
        text: read.text,
        word_count: read.word_count,
        truncated: read.truncated,
    };
    Ok(ok(
        serde_json::to_value(out).map_err(|e| ToolError::Internal(e.to_string()))?
    ))
}

#[cfg(test)]
mod tests {
    use crate::core::testutil::{item, new_item, source};
    use crate::db::{Db, repo};
    use crate::mcp::server::testkit::*;
    use serde_json::json;

    #[tokio::test]
    async fn read_item_text_is_5000_chars_max_and_records_the_read() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            let mut long = new_item("long", "s", None, "2026-09-16T00:00:00.000Z");
            long.text = "ố".repeat(6000);
            repo::insert_item(c, &long)
        })
        .unwrap();
        let h = Harness::start(server_with(db.clone())).await;
        let r = h.call("read_item", json!({ "id": "long" })).await;
        assert_eq!(r.is_error, Some(false));
        let s = structured(&r);
        assert_eq!(s["text"].as_str().unwrap().chars().count(), 5000);
        assert_eq!(s["truncated"], true);
        assert_eq!(s["source"], "S");
        assert_eq!(s["wordCount"], 12);
        assert!(
            db.with(|c| repo::list_run_reads(c, RUN))
                .unwrap()
                .contains("long")
        );
    }

    #[tokio::test]
    async fn read_item_cap_error_message_matches_snapshot() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            for i in 0..46 {
                item(
                    c,
                    &format!("i{i:02}"),
                    "s",
                    None,
                    "2026-09-16T00:00:00.000Z",
                )?;
            }
            Ok(())
        })
        .unwrap();
        let h = Harness::start(server_with(db)).await;
        for i in 0..45 {
            let r = h
                .call("read_item", json!({ "id": format!("i{i:02}") }))
                .await;
            assert_eq!(r.is_error, Some(false), "read {i}");
        }
        let r = h.call("read_item", json!({ "id": "i45" })).await;
        assert_eq!(r.is_error, Some(true));
        assert_eq!(
            structured(&r)["error"],
            "Read cap of 45 reached for this run. Select from what you have read."
        );
        let unknown = h.call("read_item", json!({ "id": "nope" })).await;
        assert_eq!(
            structured(&unknown)["error"],
            "No item with id 'nope'. Use list_candidates or search_items."
        );
    }
}
