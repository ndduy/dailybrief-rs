//! One file per tool; each exposes `run(&DailyBriefServer, input) -> Result<CallToolResult, ToolError>`.

pub mod editor_notes;
pub mod fetch_sources;
pub mod get_briefing;
pub mod list_candidates;
pub mod publish_digest;
pub mod read_item;
pub mod report_feed_issue;
pub mod search_items;
pub mod select;
