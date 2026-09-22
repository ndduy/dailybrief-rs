//! What the Curator reads (`spec/m3.md` §8): the week's evidence (`feedback`) and the current
//! profile (`profile`). Read-only; the shapes serialise straight into the two tools. Topic
//! descriptions never leave the process (same rule as `get_briefing`).

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::core::time::{days_ago_iso, to_iso};
use crate::db::{Connection, DbError, repo};

/// The evidence window when nothing else is configured.
pub const WINDOW_DAYS: u32 = 7;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RatingOut {
    pub id: i64,
    pub item_id: String,
    pub title: String,
    pub source: String,
    pub topic: String,
    pub sign: String,
    pub reason: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOut {
    pub item_id: String,
    pub title: String,
    pub source: String,
    pub at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExploreHitRate {
    pub shown: i64,
    pub read: i64,
    pub rated_up: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedIssueOut {
    pub id: i64,
    pub feed_id: String,
    pub kind: String,
    pub note: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Feedback {
    pub window: Window,
    pub ratings: Vec<RatingOut>,
    pub reads: Vec<ReadOut>,
    pub explore_hit_rate: ExploreHitRate,
    pub feed_issues: Vec<FeedIssueOut>,
}

/// Everything rated, read and reported in the last `window_days`, plus how the explore slots
/// did over the same window.
pub fn feedback(
    conn: &Connection,
    now: DateTime<Utc>,
    window_days: u32,
) -> Result<Feedback, DbError> {
    let from = days_ago_iso(now, window_days);
    let ratings = repo::list_rating_details_since(conn, &from)?
        .into_iter()
        .map(|r| RatingOut {
            id: r.id,
            item_id: r.item_id,
            title: r.title,
            source: r.source,
            topic: r.topic,
            sign: r.sign,
            reason: r.reason,
            at: r.at,
        })
        .collect();
    let reads = repo::list_read_details_since(conn, &from)?
        .into_iter()
        .map(|r| ReadOut {
            item_id: r.item_id,
            title: r.title,
            source: r.source,
            at: r.at,
        })
        .collect();
    let stats = repo::explore_stats_since(conn, &from)?;
    let feed_issues = repo::list_feed_issues_since(conn, &from)?
        .into_iter()
        .map(|f| FeedIssueOut {
            id: f.id,
            feed_id: f.source_id,
            kind: f.kind,
            note: f.note,
            at: f.at,
        })
        .collect();
    Ok(Feedback {
        window: Window {
            from,
            to: to_iso(now),
        },
        ratings,
        reads,
        explore_hit_rate: ExploreHitRate {
            shown: stats.shown,
            read: stats.read,
            rated_up: stats.rated_up,
        },
        feed_issues,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicOut {
    pub id: String,
    pub name: String,
    pub weight: f64,
    pub origin: String,
    pub saturation: i64,
    pub last_positive_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceOut {
    pub id: String,
    pub title: String,
    pub url: String,
    pub weight: f64,
    pub enabled: bool,
    pub failures: i64,
    pub last_ok_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub topics: Vec<TopicOut>,
    pub sources: Vec<SourceOut>,
}

/// Topics (ids, names, weights, origins, saturation; no descriptions) and sources with health.
pub fn profile(conn: &Connection) -> Result<Profile, DbError> {
    let topics = repo::list_topics(conn)?
        .into_iter()
        .map(|t| TopicOut {
            id: t.id,
            name: t.name,
            weight: t.weight,
            origin: t.origin.as_str().to_string(),
            saturation: t.saturation,
            last_positive_at: t.last_positive_at,
        })
        .collect();
    let sources = repo::list_sources(conn)?
        .into_iter()
        .map(|s| SourceOut {
            id: s.id,
            title: s.title,
            url: s.url,
            weight: s.weight,
            enabled: s.enabled,
            failures: s.failures,
            last_ok_at: s.last_ok_at,
        })
        .collect();
    Ok(Profile { topics, sources })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::testutil::{item, source};
    use crate::db::Db;
    use crate::db::repo::{DigestInsert, DigestItemInsert, NewRun, Role, RunKind, Section};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 17, 6, 0, 0).unwrap()
    }

    /// One digest with `fy` in for_you and `br` in beyond_radar, published at `at`.
    fn digest(c: &Connection, id: &str, fy: &str, br: &str, at: &str) -> Result<(), DbError> {
        repo::insert_run(
            c,
            &NewRun {
                id: format!("run-{id}"),
                kind: RunKind::Scheduled,
                role: Role::Editor,
                harness: "test".into(),
                attempt: 1,
                started_at: at.into(),
                transcript_path: None,
            },
        )?;
        let card = |item: &str, section: Section, topic: &str| DigestItemInsert {
            item_id: item.into(),
            section,
            position: 1,
            summary: "s".into(),
            why_it_matters: "w".into(),
            reason: matches!(section, Section::BeyondRadar).then(|| "adjacent_field".to_string()),
            topic: topic.into(),
        };
        repo::insert_digest(
            c,
            &DigestInsert {
                id: id.into(),
                date: at[..10].into(),
                run_id: format!("run-{id}"),
                published_at: at.into(),
                for_you_count: 1,
                beyond_radar_count: 1,
            },
            &[
                card(fy, Section::ForYou, "Rust"),
                card(br, Section::BeyondRadar, "Fungi"),
            ],
        )
    }

    #[test]
    fn explore_hit_rate_counts_beyond_radar_only() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            for id in ["fy1", "br1", "fy2", "br2", "old", "old2"] {
                item(c, id, "s", None, "2026-09-15T00:00:00.000Z")?;
            }
            digest(c, "d1", "fy1", "br1", "2026-09-15T00:00:00.000Z")?;
            digest(c, "d2", "fy2", "br2", "2026-09-16T00:00:00.000Z")?;
            digest(c, "d0", "old", "old2", "2026-09-01T00:00:00.000Z")?;
            // Reads and 👍s on for_you items do not count; br1 read and liked, br2 untouched.
            repo::insert_read(c, "fy1", Some("d1"), "2026-09-15T01:00:00.000Z")?;
            repo::upsert_rating(
                c,
                "fy2",
                Some("d2"),
                "up",
                "good_source",
                "2026-09-16T01:00:00.000Z",
            )?;
            repo::insert_read(c, "br1", Some("d1"), "2026-09-15T02:00:00.000Z")?;
            repo::upsert_rating(
                c,
                "br1",
                Some("d1"),
                "up",
                "new_to_me",
                "2026-09-15T02:01:00.000Z",
            )?;
            repo::insert_read(c, "old", Some("d0"), "2026-09-01T02:00:00.000Z")?;
            Ok(())
        })
        .unwrap();
        let fb = db.with(|c| feedback(c, now(), WINDOW_DAYS)).unwrap();
        assert_eq!(
            fb.explore_hit_rate,
            ExploreHitRate {
                shown: 2,
                read: 1,
                rated_up: 1
            }
        );
    }

    #[test]
    fn feedback_window_excludes_old_rows_and_carries_titles_sources_and_topics() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            for id in ["a", "b", "c"] {
                item(c, id, "s", None, "2026-09-15T00:00:00.000Z")?;
            }
            digest(c, "d1", "a", "b", "2026-09-15T00:00:00.000Z")?;
            repo::upsert_rating(
                c,
                "a",
                Some("d1"),
                "down",
                "off_topic",
                "2026-09-15T01:00:00.000Z",
            )?;
            repo::upsert_rating(c, "b", None, "up", "new_to_me", "2026-09-16T01:00:00.000Z")?;
            repo::upsert_rating(c, "c", None, "up", "new_to_me", "2026-09-01T01:00:00.000Z")?;
            repo::insert_read(c, "a", Some("d1"), "2026-09-15T01:00:00.000Z")?;
            repo::insert_read(c, "c", None, "2026-09-01T01:00:00.000Z")?;
            repo::insert_feed_issue(
                c,
                "s",
                "run-d1",
                "paywalled",
                Some("half the posts"),
                "2026-09-15T00:30:00.000Z",
            )?;
            repo::insert_feed_issue(c, "s", "run-d1", "dead", None, "2026-09-01T00:30:00.000Z")?;
            Ok(())
        })
        .unwrap();
        let fb = db.with(|c| feedback(c, now(), WINDOW_DAYS)).unwrap();
        assert_eq!(fb.window.from, "2026-09-10T06:00:00.000Z");
        assert_eq!(fb.window.to, "2026-09-17T06:00:00.000Z");
        let ratings: Vec<(&str, &str, &str, &str)> = fb
            .ratings
            .iter()
            .map(|r| {
                (
                    r.item_id.as_str(),
                    r.topic.as_str(),
                    r.sign.as_str(),
                    r.reason.as_str(),
                )
            })
            .collect();
        assert_eq!(
            ratings,
            vec![
                ("b", "", "up", "new_to_me"),
                ("a", "Rust", "down", "off_topic")
            ],
            "newest first; the topic comes from the digest the rating was made on"
        );
        assert_eq!(fb.ratings[1].title, "Title a");
        assert_eq!(fb.ratings[1].source, "S");
        assert_eq!(fb.reads.len(), 1);
        assert_eq!(fb.reads[0].item_id, "a");
        assert_eq!(fb.feed_issues.len(), 1);
        assert_eq!(fb.feed_issues[0].kind, "paywalled");
        assert_eq!(fb.feed_issues[0].feed_id, "s");
        let json = serde_json::to_value(&fb).unwrap();
        for key in ["window", "ratings", "reads", "exploreHitRate", "feedIssues"] {
            assert!(json.get(key).is_some(), "{key}");
        }
        assert!(json["ratings"][0].get("itemId").is_some());
        assert!(json["exploreHitRate"].get("ratedUp").is_some());
        assert!(json["feedIssues"][0].get("feedId").is_some());
    }

    #[test]
    fn profile_lists_topics_without_descriptions_and_sources_with_health() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            repo::upsert_topic(
                c,
                &crate::config::Topic {
                    id: "rust".into(),
                    name: "Rust".into(),
                    description: "SECRET-DESCRIPTION-TEXT".into(),
                    weight: 1.5,
                    origin: crate::config::TopicOrigin::Curator,
                },
            )?;
            source(c, "s")?;
            repo::mark_source_failed(c, "s", "HTTP 500")?;
            Ok(())
        })
        .unwrap();
        let p = db.with(|c| profile(c)).unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("SECRET-DESCRIPTION-TEXT"));
        assert_eq!(p.topics[0].origin, "curator");
        assert_eq!(p.topics[0].weight, 1.5);
        assert_eq!(p.sources[0].failures, 1);
        assert!(p.sources[0].enabled);
        assert!(json.contains("lastOkAt") && json.contains("lastPositiveAt"));
    }
}
