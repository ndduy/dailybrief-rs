//! The five Curator tools (`spec/m3.md` curator-tools): `get_feedback` and `get_profile` wrap
//! `core::curator_input`; `find_feeds`, `validate_feed` and `propose_change` follow in Tasks 7–8
//! (typed "not available" until then).

use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::core::curator_input;
use crate::core::feedback::Evidence;
use crate::mcp::error::{ToolError, ok};
use crate::mcp::server::DailyBriefServer;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UrlInput {
    /// An absolute http(s) URL.
    pub url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProposeChangeInput {
    /// One of: topic_weight, add_topic, disable_source, add_source, promote_explore_topic.
    pub kind: String,
    /// The payload for that kind (see the prompt for the fields and bounds).
    pub payload: Value,
    /// Why: a one-paragraph summary, the rating ids, the read count, the feed issue ids, notes.
    pub evidence: Evidence,
}

pub async fn get_feedback(s: &DailyBriefServer) -> Result<CallToolResult, ToolError> {
    let now = (s.now)();
    let fb =
        s.db.call(move |conn| curator_input::feedback(conn, now, curator_input::WINDOW_DAYS))
            .await?;
    Ok(ok(
        serde_json::to_value(fb).map_err(|e| ToolError::Internal(e.to_string()))?
    ))
}

pub async fn get_profile(s: &DailyBriefServer) -> Result<CallToolResult, ToolError> {
    let p = s.db.call(|conn| curator_input::profile(conn)).await?;
    Ok(ok(
        serde_json::to_value(p).map_err(|e| ToolError::Internal(e.to_string()))?
    ))
}

pub async fn find_feeds(_s: &DailyBriefServer, _i: UrlInput) -> Result<CallToolResult, ToolError> {
    Err(ToolError::NotImplemented("find_feeds"))
}

pub async fn validate_feed(
    _s: &DailyBriefServer,
    _i: UrlInput,
) -> Result<CallToolResult, ToolError> {
    Err(ToolError::NotImplemented("validate_feed"))
}

pub async fn propose_change(
    _s: &DailyBriefServer,
    _i: ProposeChangeInput,
) -> Result<CallToolResult, ToolError> {
    Err(ToolError::NotImplemented("propose_change"))
}
