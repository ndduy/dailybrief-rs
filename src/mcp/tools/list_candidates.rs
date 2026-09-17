//! `list_candidates({ strategy, limit })` → `{ candidates: [{ id, score, title, source, published,
//! snippet, note? }] }` (`SPEC.md` §5 strategies).

use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::core::candidates::{Strategy, list_candidates};
use crate::mcp::error::{ToolError, ok};
use crate::mcp::server::DailyBriefServer;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListCandidatesInput {
    /// exploit (closest to your profile) | cold_topic (topics with no recent signal) |
    /// popular_unmatched (low profile match, ordered by source weight then recency)
    pub strategy: Strategy,
    /// How many to return, 1–100 (default 30).
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    30
}

pub async fn run(
    s: &DailyBriefServer,
    input: ListCandidatesInput,
) -> Result<CallToolResult, ToolError> {
    let caps = s.config.caps;
    let now = (s.now)();
    let candidates = s
        .db
        .call(move |conn| list_candidates(conn, &caps, input.strategy, input.limit as usize, now))
        .await?;
    Ok(ok(json!({ "candidates": candidates })))
}

#[cfg(test)]
mod tests {
    use crate::core::testutil::{item, source, topic};
    use crate::core::vector::DIMENSIONS;
    use crate::db::{Db, repo};
    use crate::mcp::server::testkit::*;
    use serde_json::json;

    fn seeded() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            topic(c, "t", "T")?;
            repo::set_topic_vector(c, "t", &vec![1.0; DIMENSIONS])?;
            for i in 0..3 {
                item(
                    c,
                    &format!("i{i}"),
                    "s",
                    Some(vec![1.0; DIMENSIONS]),
                    "2026-09-16T00:00:00.000Z",
                )?;
            }
            Ok(())
        })
        .unwrap();
        db
    }

    #[tokio::test]
    async fn list_candidates_exploit_shape() {
        let h = Harness::start(server_with(seeded())).await;
        let r = h
            .call(
                "list_candidates",
                json!({ "strategy": "exploit", "limit": 2 }),
            )
            .await;
        assert_eq!(r.is_error, Some(false));
        let s = structured(&r);
        let cands = s["candidates"].as_array().unwrap();
        assert_eq!(cands.len(), 2);
        for key in ["id", "score", "title", "source", "published", "snippet"] {
            assert!(cands[0].get(key).is_some(), "missing {key}: {}", cands[0]);
        }
        assert!(cands[0].get("note").is_none(), "exploit carries no note");
        let pu = structured(
            &h.call(
                "list_candidates",
                json!({ "strategy": "popular_unmatched" }),
            )
            .await,
        );
        assert!(
            pu["candidates"][0]["note"]
                .as_str()
                .unwrap()
                .starts_with("no social signal")
        );
    }

    #[tokio::test]
    async fn list_candidates_rejects_unknown_strategy_at_schema_level() {
        let h = Harness::start(server_with(seeded())).await;
        let r = h
            .call(
                "list_candidates",
                json!({ "strategy": "random", "limit": 5 }),
            )
            .await;
        assert_eq!(r.is_error, Some(true), "{r:?}");
        let tools = h.client.list_all_tools().await.unwrap();
        let t = tools.iter().find(|t| t.name == "list_candidates").unwrap();
        let schema = serde_json::to_string(&t.input_schema).unwrap();
        assert!(
            schema.contains("cold_topic") && schema.contains("popular_unmatched"),
            "{schema}"
        );
    }
}
