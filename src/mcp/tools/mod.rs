//! One file per tool; each exposes `run(&DailyBriefServer, input) -> Result<CallToolResult, ToolError>`.

pub mod get_briefing;
