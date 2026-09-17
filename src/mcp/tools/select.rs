//! `select({ section, itemId, summary, whyItMatters, reason?, topic })` → `{ ok, replaced,
//! staged: { forYou, beyondRadar } }`. `section: "none"` un-stages. Every rejection is one
//! sentence from `core::staging`.

use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::core::staging::{SelectInput, stage_selection};
use crate::db::repo::Section;
use crate::mcp::error::{ToolError, ok};
use crate::mcp::server::DailyBriefServer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectSection {
    ForYou,
    BeyondRadar,
    /// Drop the staged row for this item.
    None,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectToolInput {
    /// for_you | beyond_radar | none (none un-stages the item)
    pub section: SelectSection,
    /// The item id you read with read_item.
    pub item_id: String,
    /// At most 80 words, in your own words.
    #[serde(default)]
    pub summary: String,
    /// At most 25 words: why this matters to the reader today.
    #[serde(default)]
    pub why_it_matters: String,
    /// Required for beyond_radar: new_to_me | adjacent_field | contrarian | deep_dive | emerging
    #[serde(default)]
    pub reason: Option<String>,
    /// One of the topic names from get_briefing.
    #[serde(default)]
    pub topic: String,
}

pub async fn run(
    s: &DailyBriefServer,
    input: SelectToolInput,
) -> Result<CallToolResult, ToolError> {
    let caps = s.config.caps;
    let run_id = s.run_id.clone();
    let now = (s.now)();
    let item_id = input.item_id.trim().to_string();
    let staged = match input.section {
        SelectSection::None => SelectInput::Unstage { item_id },
        SelectSection::ForYou | SelectSection::BeyondRadar => SelectInput::Stage {
            section: if input.section == SelectSection::ForYou {
                Section::ForYou
            } else {
                Section::BeyondRadar
            },
            item_id,
            summary: input.summary,
            why_it_matters: input.why_it_matters,
            reason: input.reason,
            topic: input.topic,
        },
    };
    let outcome =
        s.db.call(move |conn| Ok(stage_selection(conn, &caps, &run_id, now, staged)))
            .await??;
    Ok(ok(json!({
        "ok": true,
        "replaced": outcome.replaced,
        "staged": { "forYou": outcome.staged.for_you, "beyondRadar": outcome.staged.beyond_radar },
    })))
}
