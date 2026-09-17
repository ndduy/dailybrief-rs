//! `get_briefing()` → `{ date, timezone, profile: { topics: [{ name, weight }] }, shownIds,
//! feedHealth: [{ source, lastOk, failures }], notes, caps }`. Topic descriptions never leave.

use rmcp::model::CallToolResult;
use serde::Serialize;

use crate::core::time::{date_in_zone, days_ago_iso, parse_tz};
use crate::db::repo;
use crate::mcp::error::{ToolError, ok};
use crate::mcp::server::DailyBriefServer;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TopicOut {
    name: String,
    weight: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeedHealth {
    id: String,
    source: String,
    last_ok: Option<String>,
    failures: i64,
    enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CapsOut {
    reads: u32,
    for_you: u32,
    beyond_radar: u32,
    per_source: u32,
    per_topic: u32,
    web_search: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Briefing {
    date: String,
    timezone: String,
    profile: Profile,
    shown_ids: Vec<String>,
    feed_health: Vec<FeedHealth>,
    notes: String,
    caps: CapsOut,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Profile {
    topics: Vec<TopicOut>,
}

pub async fn run(s: &DailyBriefServer) -> Result<CallToolResult, ToolError> {
    let now = (s.now)();
    let tz =
        parse_tz(&s.config.service.timezone).map_err(|e| ToolError::Internal(e.to_string()))?;
    let shown_since = days_ago_iso(now, s.config.caps.shown_days);
    let (topics, shown, sources, notes) =
        s.db.call(move |conn| {
            let topics = repo::list_topics(conn)?;
            let mut shown: Vec<String> = repo::shown_item_ids(conn, &shown_since)?
                .into_iter()
                .collect();
            shown.sort();
            let sources = repo::list_sources(conn)?;
            let notes = repo::get_editor_notes(conn)?;
            Ok((topics, shown, sources, notes))
        })
        .await?;
    let caps = s.config.caps;
    let briefing = Briefing {
        date: date_in_zone(now, tz),
        timezone: s.config.service.timezone.clone(),
        profile: Profile {
            topics: topics
                .into_iter()
                .map(|t| TopicOut {
                    name: t.name,
                    weight: t.weight,
                })
                .collect(),
        },
        shown_ids: shown,
        feed_health: sources
            .into_iter()
            .map(|src| FeedHealth {
                id: src.id,
                source: src.title,
                last_ok: src.last_ok_at,
                failures: src.failures,
                enabled: src.enabled,
            })
            .collect(),
        notes,
        caps: CapsOut {
            reads: caps.reads,
            for_you: caps.for_you,
            beyond_radar: caps.beyond_radar,
            per_source: caps.per_source,
            per_topic: caps.per_topic,
            web_search: caps.web_search,
        },
    };
    Ok(ok(
        serde_json::to_value(briefing).map_err(|e| ToolError::Internal(e.to_string()))?
    ))
}

#[cfg(test)]
mod tests {
    use crate::config::{Topic, TopicOrigin};
    use crate::core::testutil::{item, shown, source};
    use crate::db::{Db, repo};
    use crate::mcp::server::testkit::*;
    use serde_json::json;

    #[tokio::test]
    async fn get_briefing_returns_shape_and_omits_topic_descriptions() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            repo::upsert_topic(
                c,
                &Topic {
                    id: "rust".into(),
                    name: "Rust".into(),
                    description: "SECRET-DESCRIPTION-TEXT".into(),
                    weight: 1.5,
                    origin: TopicOrigin::Seed,
                },
            )?;
            source(c, "s")?;
            repo::mark_source_failed(c, "s", "HTTP 500")?;
            item(c, "seen", "s", None, "2026-09-10T00:00:00.000Z")?;
            shown(c, "seen", "2026-09-10T00:00:00.000Z")?;
            repo::set_editor_notes(c, "remember the cats", "2026-09-16T00:00:00.000Z")
        })
        .unwrap();
        let h = Harness::start(server_with(db)).await;
        let r = h.call("get_briefing", json!({})).await;
        assert_eq!(r.is_error, Some(false));
        let s = structured(&r);
        assert_eq!(s["date"], "2026-09-17");
        assert_eq!(s["timezone"], "Asia/Ho_Chi_Minh");
        assert_eq!(
            s["profile"]["topics"],
            json!([{ "name": "Rust", "weight": 1.5 }])
        );
        assert_eq!(s["shownIds"], json!(["seen"]));
        assert_eq!(s["feedHealth"][0]["source"], "S");
        assert_eq!(s["feedHealth"][0]["failures"], 1);
        assert_eq!(s["notes"], "remember the cats");
        assert_eq!(s["caps"]["forYou"], 24);
        assert!(!s.to_string().contains("SECRET-DESCRIPTION-TEXT"));
    }
}
