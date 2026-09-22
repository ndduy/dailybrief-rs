//! The five Curator tools (`spec/m3.md` curator-tools). Tasks 6–8 fill these in; until then
//! each answers a typed "not available" so the listing and the role wiring can be tested.

use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::core::feedback::Evidence;
use crate::mcp::error::ToolError;
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

pub async fn get_feedback(_s: &DailyBriefServer) -> Result<CallToolResult, ToolError> {
    Err(ToolError::NotImplemented("get_feedback"))
}

pub async fn get_profile(_s: &DailyBriefServer) -> Result<CallToolResult, ToolError> {
    Err(ToolError::NotImplemented("get_profile"))
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
