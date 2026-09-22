//! Candidate strategies for `list_candidates` and the embedding search behind `search_items`
//! (`SPEC.md` §5 "Candidate strategies, precisely"). Each returns a ranked shortlist with a
//! score; the Editor decides.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::profile::{exploit_score, profile_vectors};
use super::time::days_ago_iso;
use super::vector::cosine;
use crate::config::Caps;
use crate::db::repo::Item;
use crate::db::{Connection, DbError, repo};

pub const MAX_LIMIT: usize = 100;
pub const SEARCH_MAX_LIMIT: usize = 30;
pub const SNIPPET_CHARS: usize = 200;
/// A topic with no positive signal for this many days is "cold".
pub const COLD_TOPIC_DAYS: u32 = 30;
/// `cold_topic` never returns anything in the exploit top-N.
pub const COLD_EXCLUDE_TOP: usize = 80;
/// `popular_unmatched` keeps items at or below this exploit percentile.
pub const POPULAR_PERCENTILE: f32 = 0.4;
pub const POPULAR_UNMATCHED_NOTE: &str =
    "no social signal yet: low-exploit items ordered by source weight, then recency";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    Exploit,
    ColdTopic,
    PopularUnmatched,
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

/// Ascending sort, index `ceil(p·n) − 1` clamped into `0..n`; 0 for an empty set.
pub fn percentile(values: &[f32], p: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let raw = (p * n as f32).ceil() as isize - 1;
    let index = raw.clamp(0, n as isize - 1) as usize;
    sorted[index]
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

fn source_titles(conn: &Connection) -> Result<HashMap<String, (String, f64)>, DbError> {
    Ok(repo::list_sources(conn)?
        .into_iter()
        .map(|s| (s.id, (s.title, s.weight)))
        .collect())
}

fn to_candidate(
    sources: &HashMap<String, (String, f64)>,
    item: Item,
    score: f32,
    note: Option<&str>,
) -> Candidate {
    let source = sources
        .get(&item.source_id)
        .map_or_else(|| item.source_id.clone(), |(title, _)| title.clone());
    Candidate {
        id: item.id,
        score,
        title: item.title,
        source,
        published: item.published_at,
        snippet: snippet(&item.text),
        note: note.map(str::to_string),
    }
}

fn exploit_top_ids(scored: &[Scored], n: usize) -> HashSet<String> {
    let mut ranked: Vec<(&str, f32)> = scored
        .iter()
        .map(|s| (s.item.id.as_str(), s.exploit))
        .collect();
    ranked.sort_by(|a, b| by_score_then_id(*a, *b));
    ranked
        .into_iter()
        .take(n)
        .map(|(id, _)| id.to_string())
        .collect()
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
    let sources = source_titles(conn)?;
    let mut scored = pool(conn, caps, now)?;
    match strategy {
        Strategy::Exploit => {
            scored
                .sort_by(|a, b| by_score_then_id((&a.item.id, a.exploit), (&b.item.id, b.exploit)));
            Ok(scored
                .into_iter()
                .take(limit)
                .map(|s| to_candidate(&sources, s.item, s.exploit, None))
                .collect())
        }
        Strategy::ColdTopic => {
            let cold_since = days_ago_iso(now, COLD_TOPIC_DAYS);
            let cold: Vec<Vec<f32>> = repo::list_topics(conn)?
                .into_iter()
                .filter(|t| {
                    t.last_positive_at
                        .as_deref()
                        .is_none_or(|at| at < cold_since.as_str())
                })
                .filter_map(|t| t.vector)
                .collect();
            let excluded = exploit_top_ids(&scored, COLD_EXCLUDE_TOP);
            let mut rest: Vec<(Item, f32)> = scored
                .into_iter()
                .filter(|s| !excluded.contains(&s.item.id))
                .map(|s| {
                    let score = s.item.vector.as_deref().map_or(0.0, |v| {
                        cold.iter().map(|t| cosine(v, t)).fold(0.0, f32::max)
                    });
                    (s.item, score)
                })
                .collect();
            rest.sort_by(|a, b| by_score_then_id((&a.0.id, a.1), (&b.0.id, b.1)));
            Ok(rest
                .into_iter()
                .take(limit)
                .map(|(item, score)| to_candidate(&sources, item, score, None))
                .collect())
        }
        Strategy::PopularUnmatched => {
            let cutoff = percentile(
                &scored.iter().map(|s| s.exploit).collect::<Vec<_>>(),
                POPULAR_PERCENTILE,
            );
            let weight = |item: &Item| sources.get(&item.source_id).map_or(0.0, |(_, w)| *w);
            let mut low: Vec<Scored> = scored.into_iter().filter(|s| s.exploit <= cutoff).collect();
            low.sort_by(|a, b| {
                weight(&b.item)
                    .partial_cmp(&weight(&a.item))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        b.item
                            .published_or_fetched()
                            .cmp(a.item.published_or_fetched())
                    })
                    .then_with(|| a.item.id.cmp(&b.item.id))
            });
            Ok(low
                .into_iter()
                .take(limit)
                .map(|s| to_candidate(&sources, s.item, s.exploit, Some(POPULAR_UNMATCHED_NOTE)))
                .collect())
        }
    }
}

/// Cosine of an already-embedded query over the candidate window; shown items are included
/// because the Editor sees `shownIds` and may search for context.
pub fn search_items(
    conn: &Connection,
    caps: &Caps,
    query: &[f32],
    limit: usize,
    now: DateTime<Utc>,
) -> Result<Vec<Candidate>, DbError> {
    let limit = clamp_limit(limit, SEARCH_MAX_LIMIT);
    let sources = source_titles(conn)?;
    let window_start = days_ago_iso(now, caps.candidate_days);
    let mut hits: Vec<(Item, f32)> = repo::list_items_since(conn, &window_start)?
        .into_iter()
        .filter_map(|item| {
            let score = cosine(query, item.vector.as_deref()?);
            Some((item, score))
        })
        .collect();
    hits.sort_by(|a, b| by_score_then_id((&a.0.id, a.1), (&b.0.id, b.1)));
    Ok(hits
        .into_iter()
        .take(limit)
        .map(|(item, score)| to_candidate(&sources, item, score, None))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Env, Feed, Topic, TopicOrigin, load_config};
    use crate::core::testutil::{item, source};
    use crate::core::vector::DIMENSIONS;
    use crate::db::Db;
    use chrono::TimeZone;

    const RECENT: &str = "2026-09-16T00:00:00.000Z";

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

    fn topic(conn: &Connection, id: &str, v: Vec<f32>, last_positive_at: Option<&str>) {
        repo::upsert_topic(
            conn,
            &Topic {
                id: id.into(),
                name: id.to_uppercase(),
                description: String::new(),
                weight: 1.0,
                origin: TopicOrigin::Seed,
            },
        )
        .unwrap();
        repo::set_topic_vector(conn, id, &v).unwrap();
        if let Some(at) = last_positive_at {
            conn.execute(
                "UPDATE topics SET last_positive_at = ?2 WHERE id = ?1",
                [id, at],
            )
            .unwrap();
        }
    }

    fn db_with_topic() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            topic(c, "t", unit(0), Some(RECENT));
            Ok(())
        })
        .unwrap();
        db
    }

    fn ids(cands: &[Candidate]) -> Vec<&str> {
        cands.iter().map(|c| c.id.as_str()).collect()
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
            item(c, "b", "s", Some(mixed(1.0, 0.0)), RECENT)?;
            item(c, "a", "s", Some(mixed(1.0, 0.0)), RECENT)?;
            item(c, "z", "s", Some(mixed(0.2, 1.0)), RECENT)?;
            Ok(())
        })
        .unwrap();
        let got = db
            .with(|c| list_candidates(c, &caps(), Strategy::Exploit, 500, now()))
            .unwrap();
        assert_eq!(ids(&got), vec!["a", "b", "z"]);
        assert!(got[0].score > got[2].score);
        assert_eq!(got[0].source, "S");
        assert!(got[0].note.is_none());
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

    /// `a` sits on the topic axis, `c` between the topic and the rated direction, `d` on a
    /// third axis; `r` (the rated item) is too old to be a candidate itself.
    fn rating_fixture() -> Db {
        let db = db_with_topic();
        db.with(|c| {
            item(c, "a", "s", Some(unit(0)), RECENT)?;
            item(c, "c", "s", Some(mixed(0.3, 0.3)), RECENT)?;
            item(c, "d", "s", Some(unit(2)), RECENT)?;
            item(c, "r", "s", Some(unit(1)), "2026-08-01T00:00:00.000Z")?;
            Ok(())
        })
        .unwrap();
        db
    }

    fn exploit_order(db: &Db) -> Vec<(String, f32)> {
        db.with(|c| list_candidates(c, &caps(), Strategy::Exploit, 500, now()))
            .unwrap()
            .into_iter()
            .map(|c| (c.id, c.score))
            .collect()
    }

    #[test]
    fn a_positive_rating_moves_an_items_rank_in_exploit() {
        let db = rating_fixture();
        let before = exploit_order(&db);
        assert_eq!(
            before.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            vec!["a", "c", "d"]
        );
        db.with(|c| repo::upsert_rating(c, "r", None, "up", "new_to_me", RECENT))
            .unwrap();
        let after = exploit_order(&db);
        assert_eq!(
            after.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            vec!["c", "a", "d"],
            "c, which shares the rated direction, climbs past a; d stays last"
        );
        assert!(
            after.iter().all(|(id, _)| id != "r"),
            "the rated item is not a candidate"
        );
    }

    #[test]
    fn a_negative_rating_changes_no_score() {
        let db = rating_fixture();
        let before = exploit_order(&db);
        db.with(|c| repo::upsert_rating(c, "r", None, "down", "off_topic", RECENT))
            .unwrap();
        assert_eq!(exploit_order(&db), before);
    }

    #[test]
    fn exploit_excludes_shown_items() {
        let db = db_with_topic();
        db.with(|c| {
            item(c, "shown", "s", Some(unit(0)), RECENT)?;
            item(c, "fresh", "s", Some(unit(0)), RECENT)?;
            crate::core::testutil::shown(c, "shown", "2026-09-10T00:00:00.000Z")
        })
        .unwrap();
        let got = db
            .with(|c| list_candidates(c, &caps(), Strategy::Exploit, 10, now()))
            .unwrap();
        assert_eq!(ids(&got), vec!["fresh"]);
    }

    #[test]
    fn exploit_excludes_items_outside_window() {
        let db = db_with_topic();
        db.with(|c| {
            let mut old = crate::core::testutil::new_item("old_pub", "s", Some(unit(0)), RECENT);
            old.published_at = Some("2026-08-01T00:00:00.000Z".into());
            repo::insert_item(c, &old)?;
            item(
                c,
                "old_fetch",
                "s",
                Some(unit(0)),
                "2026-08-01T00:00:00.000Z",
            )?;
            item(c, "novec", "s", None, RECENT)?;
            item(c, "ok", "s", Some(unit(0)), RECENT)?;
            Ok(())
        })
        .unwrap();
        let got = db
            .with(|c| list_candidates(c, &caps(), Strategy::Exploit, 10, now()))
            .unwrap();
        assert_eq!(ids(&got), vec!["ok"]);
    }

    /// 85 items leaning to the hot topic plus one cold hit: the exploit top-80 are all hot items.
    fn cold_fixture() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            topic(c, "hot", unit(0), Some(RECENT));
            topic(c, "cold", unit(1), None);
            topic(c, "stale", unit(2), Some("2026-07-01T00:00:00.000Z"));
            for i in 0..85 {
                item(c, &format!("hot{i:02}"), "s", Some(mixed(1.0, 0.1)), RECENT)?;
            }
            item(c, "coldhit", "s", Some(unit(1)), RECENT)?;
            item(c, "stalehit", "s", Some(unit(2)), RECENT)?;
            Ok(())
        })
        .unwrap();
        db
    }

    #[test]
    fn cold_topic_uses_only_cold_topics() {
        let db = cold_fixture();
        let got = db
            .with(|c| list_candidates(c, &caps(), Strategy::ColdTopic, 100, now()))
            .unwrap();
        assert_eq!(got[0].id, "coldhit");
        assert!((got[0].score - 1.0).abs() < 1e-6);
        assert_eq!(
            got[1].id, "stalehit",
            "a topic positive 78 days ago is cold too"
        );
        assert!((got[1].score - 1.0).abs() < 1e-6);
        let hot_scores: Vec<f32> = got
            .iter()
            .filter(|c| c.id.starts_with("hot"))
            .map(|c| c.score)
            .collect();
        assert!(
            hot_scores.iter().all(|s| *s < 0.2),
            "hot items score low against cold topics: {hot_scores:?}"
        );
    }

    #[test]
    fn cold_topic_excludes_exploit_top_80() {
        let db = cold_fixture();
        let top80: HashSet<String> = db
            .with(|c| list_candidates(c, &caps(), Strategy::Exploit, 80, now()))
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(top80.len(), 80);
        let cold = db
            .with(|c| list_candidates(c, &caps(), Strategy::ColdTopic, 100, now()))
            .unwrap();
        assert_eq!(cold.len(), 87 - 80);
        assert!(cold.iter().all(|c| !top80.contains(&c.id)));
    }

    #[test]
    fn popular_unmatched_percentile_formula() {
        assert_eq!(percentile(&[], 0.4), 0.0);
        assert_eq!(percentile(&[7.0], 0.4), 7.0);
        // n = 5: ceil(0.4·5) − 1 = 1 → second smallest
        assert_eq!(percentile(&[5.0, 1.0, 4.0, 2.0, 3.0], 0.4), 2.0);
        // n = 10: ceil(4) − 1 = 3 → fourth smallest
        let ten: Vec<f32> = (1..=10).rev().map(|i| i as f32).collect();
        assert_eq!(percentile(&ten, 0.4), 4.0);
        assert_eq!(percentile(&ten, 0.0), 1.0);
        assert_eq!(percentile(&ten, 1.0), 10.0);
    }

    #[test]
    fn popular_unmatched_orders_by_source_weight_then_recency_then_id_and_carries_note() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            for (id, w) in [("heavy", 2.0), ("light", 1.0)] {
                repo::upsert_source(
                    c,
                    &Feed {
                        id: id.into(),
                        url: format!("https://{id}.example/rss"),
                        title: id.into(),
                        weight: w,
                        enabled: true,
                    },
                )?;
            }
            // No topics: every exploit score is 0, so everything is at or below p40.
            let mut older =
                crate::core::testutil::new_item("l_old", "light", Some(unit(0)), RECENT);
            older.published_at = Some("2026-09-15T00:00:00.000Z".into());
            repo::insert_item(c, &older)?;
            let mut newer_b =
                crate::core::testutil::new_item("l_new_b", "light", Some(unit(0)), RECENT);
            newer_b.published_at = Some("2026-09-16T12:00:00.000Z".into());
            repo::insert_item(c, &newer_b)?;
            let mut newer_a =
                crate::core::testutil::new_item("l_new_a", "light", Some(unit(0)), RECENT);
            newer_a.published_at = Some("2026-09-16T12:00:00.000Z".into());
            repo::insert_item(c, &newer_a)?;
            item(c, "h", "heavy", Some(unit(0)), RECENT)?;
            Ok(())
        })
        .unwrap();
        let got = db
            .with(|c| list_candidates(c, &caps(), Strategy::PopularUnmatched, 10, now()))
            .unwrap();
        assert_eq!(ids(&got), vec!["h", "l_new_a", "l_new_b", "l_old"]);
        assert!(
            got.iter()
                .all(|c| c.note.as_deref() == Some(POPULAR_UNMATCHED_NOTE))
        );
    }

    #[test]
    fn popular_unmatched_keeps_only_items_at_or_below_p40() {
        let db = db_with_topic();
        db.with(|c| {
            for i in 0..10 {
                // exploit ≈ cosine to unit(0): i = 0 → 0, i = 9 → high
                item(c, &format!("i{i}"), "s", Some(mixed(i as f32, 1.0)), RECENT)?;
            }
            Ok(())
        })
        .unwrap();
        let got = db
            .with(|c| list_candidates(c, &caps(), Strategy::PopularUnmatched, 100, now()))
            .unwrap();
        assert_eq!(
            got.len(),
            4,
            "ceil(0.4·10) items at or below the cutoff: {:?}",
            ids(&got)
        );
    }

    #[test]
    fn search_items_includes_shown_items_and_ranks_by_cosine() {
        let db = db_with_topic();
        db.with(|c| {
            item(c, "shown", "s", Some(unit(3)), RECENT)?;
            item(c, "other", "s", Some(unit(4)), RECENT)?;
            crate::core::testutil::shown(c, "shown", "2026-09-10T00:00:00.000Z")
        })
        .unwrap();
        let got = db
            .with(|c| search_items(c, &caps(), &unit(3), 10, now()))
            .unwrap();
        assert_eq!(ids(&got), vec!["shown", "other"]);
        assert!((got[0].score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn search_items_clamps_limit_to_30() {
        let db = db_with_topic();
        db.with(|c| {
            for i in 0..35 {
                item(c, &format!("i{i:02}"), "s", Some(unit(i % 5)), RECENT)?;
            }
            Ok(())
        })
        .unwrap();
        let got = db
            .with(|c| search_items(c, &caps(), &unit(0), 100, now()))
            .unwrap();
        assert_eq!(got.len(), SEARCH_MAX_LIMIT);
    }
}
