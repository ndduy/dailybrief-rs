//! Candidate strategies for `list_candidates` (`SPEC.md` §5 "Candidate strategies, precisely").
//! Each returns a ranked shortlist with a score; the Editor decides.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::profile::{exploit_score, profile_vectors};
use super::time::days_ago_iso;
use crate::config::Caps;
use crate::db::repo::Item;
use crate::db::{Connection, DbError, repo};

pub const MAX_LIMIT: usize = 100;
pub const SNIPPET_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    Exploit,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: String,
    pub score: f32,
    pub title: String,
    pub source: String,
    pub published: Option<String>,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The first `SNIPPET_CHARS` characters (not bytes) of the whitespace-collapsed text, `…` when cut.
pub fn snippet(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = flat.chars();
    let head: String = chars.by_ref().take(SNIPPET_CHARS - 1).collect();
    match chars.next() {
        None => head,
        Some(_) => format!("{}…", head.trim_end()),
    }
}

/// Clamp a requested limit into `1..=max`.
pub fn clamp_limit(limit: usize, max: usize) -> usize {
    limit.clamp(1, max)
}

/// An item scored against the profile.
pub struct Scored {
    pub item: Item,
    pub exploit: f32,
}

/// The candidate pool: items with a vector, fetched inside `candidate_days`, whose
/// `published_at` (or `fetched_at`) is inside the window too, and not shown in `shown_days`.
pub fn pool(conn: &Connection, caps: &Caps, now: DateTime<Utc>) -> Result<Vec<Scored>, DbError> {
    let window_start = days_ago_iso(now, caps.candidate_days);
    let shown = repo::shown_item_ids(conn, &days_ago_iso(now, caps.shown_days))?;
    let profile = profile_vectors(conn)?;
    Ok(repo::list_items_since(conn, &window_start)?
        .into_iter()
        .filter(|i| {
            i.vector.is_some()
                && !shown.contains(&i.id)
                && i.published_or_fetched() >= window_start.as_str()
        })
        .map(|item| {
            let exploit = item
                .vector
                .as_deref()
                .map_or(0.0, |v| exploit_score(v, &profile));
            Scored { item, exploit }
        })
        .collect())
}

/// Score descending, then id ascending (a total order for stable output).
pub fn by_score_then_id(a: (&str, f32), b: (&str, f32)) -> std::cmp::Ordering {
    b.1.partial_cmp(&a.1)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.0.cmp(b.0))
}

fn to_candidate(conn: &Connection, item: Item, score: f32) -> Result<Candidate, DbError> {
    let source = repo::get_source(conn, &item.source_id)?
        .map_or_else(|| item.source_id.clone(), |s| s.title);
    Ok(Candidate {
        id: item.id,
        score,
        title: item.title,
        source,
        published: item.published_at,
        snippet: snippet(&item.text),
        note: None,
    })
}

/// Runs one strategy over the pool.
pub fn list_candidates(
    conn: &Connection,
    caps: &Caps,
    strategy: Strategy,
    limit: usize,
    now: DateTime<Utc>,
) -> Result<Vec<Candidate>, DbError> {
    let limit = clamp_limit(limit, MAX_LIMIT);
    let mut scored = pool(conn, caps, now)?;
    match strategy {
        Strategy::Exploit => {
            scored
                .sort_by(|a, b| by_score_then_id((&a.item.id, a.exploit), (&b.item.id, b.exploit)));
            scored
                .into_iter()
                .take(limit)
                .map(|s| to_candidate(conn, s.item, s.exploit))
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Env, Topic, TopicOrigin, load_config};
    use crate::core::testutil::{item, source};
    use crate::core::vector::DIMENSIONS;
    use crate::db::Db;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 17, 6, 0, 0).unwrap()
    }

    fn caps() -> Caps {
        load_config(&Env::from_lookup(|_| None).unwrap())
            .unwrap()
            .caps
    }

    fn unit(i: usize) -> Vec<f32> {
        let mut v = vec![0.0; DIMENSIONS];
        v[i] = 1.0;
        v
    }

    fn mixed(a: f32, b: f32) -> Vec<f32> {
        let mut v = vec![0.0; DIMENSIONS];
        v[0] = a;
        v[1] = b;
        v
    }

    fn db_with_topic() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            repo::upsert_topic(
                c,
                &Topic {
                    id: "t".into(),
                    name: "T".into(),
                    description: String::new(),
                    weight: 1.0,
                    origin: TopicOrigin::Seed,
                },
            )?;
            repo::set_topic_vector(c, "t", &unit(0))
        })
        .unwrap();
        db
    }

    #[test]
    fn snippet_cuts_on_chars_not_bytes() {
        let text = "ô ".repeat(300);
        let s = snippet(&text);
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= SNIPPET_CHARS);
        assert_eq!(snippet("  short   text "), "short text");
    }

    #[test]
    fn exploit_sorts_by_score_then_id_and_clamps_limit() {
        let db = db_with_topic();
        db.with(|c| {
            item(
                c,
                "b",
                "s",
                Some(mixed(1.0, 0.0)),
                "2026-09-16T00:00:00.000Z",
            )?;
            item(
                c,
                "a",
                "s",
                Some(mixed(1.0, 0.0)),
                "2026-09-16T00:00:00.000Z",
            )?;
            item(
                c,
                "z",
                "s",
                Some(mixed(0.2, 1.0)),
                "2026-09-16T00:00:00.000Z",
            )?;
            Ok(())
        })
        .unwrap();
        let got = db
            .with(|c| list_candidates(c, &caps(), Strategy::Exploit, 500, now()))
            .unwrap();
        let ids: Vec<&str> = got.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "z"]);
        assert!(got[0].score > got[2].score);
        assert_eq!(got[0].source, "S");
        let two = db
            .with(|c| list_candidates(c, &caps(), Strategy::Exploit, 2, now()))
            .unwrap();
        assert_eq!(two.len(), 2);
        let one = db
            .with(|c| list_candidates(c, &caps(), Strategy::Exploit, 0, now()))
            .unwrap();
        assert_eq!(one.len(), 1, "limit 0 clamps to 1");
        assert_eq!(clamp_limit(500, MAX_LIMIT), 100);
    }

    #[test]
    fn exploit_excludes_shown_items() {
        let db = db_with_topic();
        db.with(|c| {
            item(c, "shown", "s", Some(unit(0)), "2026-09-16T00:00:00.000Z")?;
            item(c, "fresh", "s", Some(unit(0)), "2026-09-16T00:00:00.000Z")?;
            crate::core::testutil::shown(c, "shown", "2026-09-10T00:00:00.000Z")
        })
        .unwrap();
        let got = db
            .with(|c| list_candidates(c, &caps(), Strategy::Exploit, 10, now()))
            .unwrap();
        assert_eq!(
            got.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["fresh"]
        );
    }

    #[test]
    fn exploit_excludes_items_outside_window() {
        let db = db_with_topic();
        db.with(|c| {
            // fetched inside the window but published long ago
            let mut old = crate::core::testutil::new_item(
                "old_pub",
                "s",
                Some(unit(0)),
                "2026-09-16T00:00:00.000Z",
            );
            old.published_at = Some("2026-08-01T00:00:00.000Z".into());
            repo::insert_item(c, &old)?;
            // fetched long ago, no published_at → fetched_at rules it out
            item(
                c,
                "old_fetch",
                "s",
                Some(unit(0)),
                "2026-08-01T00:00:00.000Z",
            )?;
            // no vector
            item(c, "novec", "s", None, "2026-09-16T00:00:00.000Z")?;
            item(c, "ok", "s", Some(unit(0)), "2026-09-16T00:00:00.000Z")?;
            Ok(())
        })
        .unwrap();
        let got = db
            .with(|c| list_candidates(c, &caps(), Strategy::Exploit, 10, now()))
            .unwrap();
        assert_eq!(
            got.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec!["ok"]
        );
    }
}
