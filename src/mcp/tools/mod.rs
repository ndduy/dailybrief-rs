//! One file per tool; each exposes `run(&DailyBriefServer, input) -> Result<CallToolResult, ToolError>`.

pub mod editor_notes;
pub mod fetch_sources;
pub mod get_briefing;
pub mod report_feed_issue;
