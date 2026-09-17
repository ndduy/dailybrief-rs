//! `search_items({ query, limit })` → the candidates shape. The query is embedded locally.

use std::sync::Arc;

use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::core::candidates::search_items;
use crate::mcp::error::{ToolError, ok};
use crate::mcp::server::DailyBriefServer;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchItemsInput {
    /// What you are looking for, in plain words.
    pub query: String,
    /// How many to return, 1–30 (default 10).
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    10
}

pub async fn run(
    s: &DailyBriefServer,
    input: SearchItemsInput,
) -> Result<CallToolResult, ToolError> {
    let query = input.query.trim().to_string();
    if query.is_empty() {
        return Ok(ok(json!({ "candidates": [] })));
    }
    let embedder = Arc::clone(&s.embedder);
    let vectors = tokio::task::spawn_blocking(move || embedder.embed(&[query]))
        .await
        .map_err(|_| ToolError::Internal("embedding task panicked".into()))?
        .map_err(|e| ToolError::Internal(e.to_string()))?;
    let Some(vector) = vectors.into_iter().next() else {
        return Ok(ok(json!({ "candidates": [] })));
    };
    let caps = s.config.caps;
    let now = (s.now)();
    let hits =
        s.db.call(move |conn| search_items(conn, &caps, &vector, input.limit as usize, now))
            .await?;
    Ok(ok(json!({ "candidates": hits })))
}

#[cfg(test)]
mod tests {
    use crate::core::embed::{Embedder, FakeEmbedder};
    use crate::core::testutil::{item, source};
    use crate::db::Db;
    use crate::mcp::server::testkit::*;
    use serde_json::json;

    #[tokio::test]
    async fn search_items_embeds_query_locally() {
        let db = Db::open_in_memory().unwrap();
        let hit_vec = FakeEmbedder
            .embed(&["borrow checker".to_string()])
            .unwrap()
            .remove(0);
        let miss_vec = FakeEmbedder
            .embed(&["sourdough".to_string()])
            .unwrap()
            .remove(0);
        db.with(|c| {
            source(c, "s")?;
            item(c, "hit", "s", Some(hit_vec), "2026-09-16T00:00:00.000Z")?;
            item(c, "miss", "s", Some(miss_vec), "2026-09-16T00:00:00.000Z")
        })
        .unwrap();
        let h = Harness::start(server_with(db)).await;
        let s = structured(
            &h.call("search_items", json!({ "query": "borrow checker" }))
                .await,
        );
        assert_eq!(s["candidates"][0]["id"], "hit");
        assert!((s["candidates"][0]["score"].as_f64().unwrap() - 1.0).abs() < 1e-5);
        let empty = structured(&h.call("search_items", json!({ "query": "   " })).await);
        assert_eq!(empty["candidates"].as_array().unwrap().len(), 0);
    }
}
