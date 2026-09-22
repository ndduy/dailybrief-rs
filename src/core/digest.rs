//! `publish_digest` (`SPEC.md` §4): validate the staged set end to end, then write `digests` +
//! `digest_items` in one transaction with positions renumbered per section.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use super::staging::EXPLORE_REASONS;
use super::time::{date_in_zone, days_ago_iso, to_iso};
use crate::config::Caps;
use crate::db::repo::{self, DigestInsert, DigestItemInsert, Section};
use crate::db::{Connection, DbError};

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("This run already published digest '{0}'.")]
    AlreadyPublished(String),
    #[error("{} violation(s)", .0.len())]
    Violations(Vec<String>),
    #[error("database: {0}")]
    Db(#[from] DbError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub digest_id: String,
    pub date: String,
    pub for_you: usize,
    pub beyond_radar: usize,
}

/// Every reason the staged set cannot be published, as one sentence each.
pub fn validate_staged(
    conn: &Connection,
    caps: &Caps,
    run_id: &str,
    now: DateTime<Utc>,
) -> Result<Vec<String>, DbError> {
    let rows = repo::list_selections(conn, run_id)?;
    let mut v = Vec::new();
    let for_you = rows.iter().filter(|r| r.section == Section::ForYou).count();
    let beyond = rows
        .iter()
        .filter(|r| r.section == Section::BeyondRadar)
        .count();
    if for_you != caps.for_you as usize {
        v.push(format!(
            "for_you has {for_you} items; exactly {} are required.",
            caps.for_you
        ));
    }
    if beyond != caps.beyond_radar as usize {
        v.push(format!(
            "beyond_radar has {beyond} items; exactly {} are required.",
            caps.beyond_radar
        ));
    }
    let shown = repo::shown_item_ids(conn, &days_ago_iso(now, caps.shown_days))?;
    let reads = repo::list_run_reads(conn, run_id)?;
    let mut per_source: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_topic: BTreeMap<String, usize> = BTreeMap::new();
    for row in &rows {
        let Some(item) = repo::get_item(conn, &row.item_id)? else {
            v.push(format!("Staged item '{}' no longer exists.", row.item_id));
            continue;
        };
        if !reads.contains(&row.item_id) {
            v.push(format!(
                "Item '{}' was staged without being read.",
                row.item_id
            ));
        }
        if shown.contains(&row.item_id) {
            v.push(format!(
                "Item '{}' was shown in the last {} days.",
                row.item_id, caps.shown_days
            ));
        }
        if row.section == Section::BeyondRadar
            && !row
                .reason
                .as_deref()
                .is_some_and(|r| EXPLORE_REASONS.contains(&r))
        {
            v.push(format!(
                "beyond_radar item '{}' has no reason.",
                row.item_id
            ));
        }
        let source =
            repo::get_source(conn, &item.source_id)?.map_or(item.source_id.clone(), |s| s.title);
        *per_source.entry(source).or_default() += 1;
        *per_topic.entry(row.topic.clone()).or_default() += 1;
    }
    for (source, n) in per_source {
        if n > caps.per_source as usize {
            v.push(format!(
                "Source '{source}' has {n} items; the cap is {} per source.",
                caps.per_source
            ));
        }
    }
    for (topic, n) in per_topic {
        if n > caps.per_topic as usize {
            v.push(format!(
                "Topic '{topic}' has {n} items; the cap is {} per topic.",
                caps.per_topic
            ));
        }
    }
    Ok(v)
}

/// Validates, then writes the digest atomically. `digest_id = <local date>-<run id>`.
pub fn publish(
    conn: &Connection,
    caps: &Caps,
    tz: Tz,
    run_id: &str,
    now: DateTime<Utc>,
) -> Result<Published, PublishError> {
    if let Some(existing) = repo::get_digest_by_run(conn, run_id)? {
        return Err(PublishError::AlreadyPublished(existing.id));
    }
    let violations = validate_staged(conn, caps, run_id, now)?;
    if !violations.is_empty() {
        return Err(PublishError::Violations(violations));
    }
    let date = date_in_zone(now, tz);
    let digest_id = format!("{date}-{run_id}");
    let rows = repo::list_selections(conn, run_id)?;
    let mut items = Vec::with_capacity(rows.len());
    for section in [Section::ForYou, Section::BeyondRadar] {
        let mut in_section: Vec<_> = rows.iter().filter(|r| r.section == section).collect();
        in_section.sort_by_key(|r| (r.position, r.item_id.clone()));
        for (i, r) in in_section.into_iter().enumerate() {
            items.push(DigestItemInsert {
                item_id: r.item_id.clone(),
                section,
                position: i as i64 + 1,
                summary: r.summary.clone(),
                why_it_matters: r.why_it_matters.clone(),
                reason: r.reason.clone(),
                topic: r.topic.clone(),
            });
        }
    }
    let for_you = items
        .iter()
        .filter(|i| i.section == Section::ForYou)
        .count();
    let beyond_radar = items.len() - for_you;
    repo::insert_digest(
        conn,
        &DigestInsert {
            id: digest_id.clone(),
            date: date.clone(),
            run_id: run_id.to_string(),
            published_at: to_iso(now),
            for_you_count: for_you as i64,
            beyond_radar_count: beyond_radar as i64,
        },
        &items,
    )?;
    Ok(Published {
        digest_id,
        date,
        for_you,
        beyond_radar,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Env, load_config};
    use crate::core::staging::{SelectInput, stage_selection};
    use crate::core::testutil::{item, shown, source, topic};
    use crate::core::time::parse_tz;
    use crate::core::vector::DIMENSIONS;
    use crate::db::Db;
    use chrono::TimeZone;

    const RECENT: &str = "2026-09-16T00:00:00.000Z";
    const RUN: &str = "2026-09-17-abcd1234";

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 17, 6, 0, 0).unwrap()
    }

    fn caps() -> Caps {
        load_config(&Env::from_lookup(|_| None).unwrap())
            .unwrap()
            .caps
    }

    fn tz() -> Tz {
        parse_tz("Asia/Ho_Chi_Minh").unwrap()
    }

    /// 8 sources × 4 items, 4 topics; all 32 items read in RUN.
    fn seeded() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            for t in 0..4 {
                topic(c, &format!("t{t}"), &format!("Topic {t}"))?;
            }
            for s in 0..8 {
                let src = format!("src{s}");
                source(c, &src)?;
                for k in 0..4 {
                    let id = format!("i{s}{k}");
                    item(c, &id, &src, Some(vec![0.1; DIMENSIONS]), RECENT)?;
                    repo::mark_run_read(c, RUN, &id)?;
                }
            }
            repo::insert_run(
                c,
                &repo::NewRun {
                    id: RUN.into(),
                    kind: repo::RunKind::Manual,
                    role: repo::Role::Editor,
                    harness: "test".into(),
                    attempt: 1,
                    started_at: RECENT.into(),
                    transcript_path: None,
                },
            )
        })
        .unwrap();
        db
    }

    fn stage_all(db: &Db, for_you: usize, beyond: usize) {
        let ids: Vec<String> = (0..8)
            .flat_map(|s| (0..4).map(move |k| format!("i{s}{k}")))
            .collect();
        db.with(|c| {
            for (n, id) in ids.iter().take(for_you + beyond).enumerate() {
                let (section, reason) = if n < for_you {
                    (Section::ForYou, None)
                } else {
                    (Section::BeyondRadar, Some("emerging".to_string()))
                };
                stage_selection(
                    c,
                    &caps(),
                    RUN,
                    now(),
                    SelectInput::Stage {
                        section,
                        item_id: id.clone(),
                        summary: format!("Summary {n}"),
                        why_it_matters: "Why.".into(),
                        reason,
                        topic: format!("Topic {}", n % 4),
                    },
                )
                .unwrap();
            }
            Ok(())
        })
        .unwrap();
    }

    fn violations(db: &Db) -> Vec<String> {
        db.with(|c| validate_staged(c, &caps(), RUN, now()))
            .unwrap()
    }

    #[test]
    fn publish_rejects_wrong_counts() {
        let db = seeded();
        stage_all(&db, 23, 6);
        assert_eq!(
            violations(&db),
            vec!["for_you has 23 items; exactly 24 are required."]
        );
        let db = seeded();
        stage_all(&db, 24, 5);
        assert_eq!(
            violations(&db),
            vec!["beyond_radar has 5 items; exactly 6 are required."]
        );
    }

    #[test]
    fn publish_rejects_unread_shown_missing_and_reasonless_rows() {
        let db = seeded();
        stage_all(&db, 24, 6);
        db.with(|c| {
            c.execute(
                "DELETE FROM run_reads WHERE run_id = ?1 AND item_id = 'i00'",
                [RUN],
            )?;
            shown(c, "i01", "2026-09-10T00:00:00.000Z")?;
            c.execute(
                "UPDATE selections SET reason = NULL WHERE run_id = ?1 AND item_id = 'i63'",
                [RUN],
            )?;
            c.execute("PRAGMA foreign_keys = OFF", [])?;
            c.execute("DELETE FROM items WHERE id = 'i70'", [])?;
            Ok(())
        })
        .unwrap();
        let v = violations(&db);
        assert!(
            v.contains(&"Item 'i00' was staged without being read.".to_string()),
            "{v:?}"
        );
        assert!(
            v.contains(&"Item 'i01' was shown in the last 14 days.".to_string()),
            "{v:?}"
        );
        assert!(
            v.contains(&"beyond_radar item 'i63' has no reason.".to_string()),
            "{v:?}"
        );
        assert!(
            v.contains(&"Staged item 'i70' no longer exists.".to_string()),
            "{v:?}"
        );
    }

    #[test]
    fn publish_rejects_source_and_topic_over_cap() {
        let db = seeded();
        stage_all(&db, 24, 6);
        // Bypass staging's incremental checks to plant an over-cap set.
        db.with(|c| {
            c.execute(
                "UPDATE selections SET topic = 'Topic 0' WHERE run_id = ?1",
                [RUN],
            )?;
            c.execute(
                "UPDATE items SET source_id = 'src0' WHERE id IN ('i10', 'i11')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        let v = violations(&db);
        assert!(
            v.contains(&"Source 'SRC0' has 6 items; the cap is 4 per source.".to_string()),
            "{v:?}"
        );
        assert!(
            v.contains(&"Topic 'Topic 0' has 30 items; the cap is 8 per topic.".to_string()),
            "{v:?}"
        );
    }

    #[test]
    fn publish_writes_atomically_renumbers_positions_and_refuses_twice() {
        let db = seeded();
        stage_all(&db, 24, 6);
        db.with(|c| {
            c.execute(
                "DELETE FROM selections WHERE run_id = ?1 AND item_id = 'i02'",
                [RUN],
            )
            .map(|_| ())
            .map_err(Into::into)
        })
        .unwrap();
        // 23 + 6 now; re-add a fresh one as for_you so positions have a gap (position 25).
        db.with(|c| {
            stage_selection(
                c,
                &caps(),
                RUN,
                now(),
                SelectInput::Stage {
                    section: Section::ForYou,
                    item_id: "i73".into(),
                    summary: "Late add".into(),
                    why_it_matters: "Why.".into(),
                    reason: None,
                    topic: "Topic 2".into(),
                },
            )
            .map(|_| ())
            .map_err(|e| DbError::Corrupt(e.to_string()))
        })
        .unwrap();
        let published = db
            .with(|c| Ok(publish(c, &caps(), tz(), RUN, now())))
            .unwrap()
            .unwrap();
        assert_eq!(published.digest_id, format!("2026-09-17-{RUN}"));
        assert_eq!(published.for_you, 24);
        assert_eq!(published.beyond_radar, 6);
        let items = db
            .with(|c| repo::list_digest_items(c, &published.digest_id))
            .unwrap();
        assert_eq!(items.len(), 30);
        let for_you_positions: Vec<i64> = items
            .iter()
            .filter(|i| i.section == Section::ForYou)
            .map(|i| i.position)
            .collect();
        assert_eq!(for_you_positions, (1..=24).collect::<Vec<_>>());
        let beyond_positions: Vec<i64> = items
            .iter()
            .filter(|i| i.section == Section::BeyondRadar)
            .map(|i| i.position)
            .collect();
        assert_eq!(beyond_positions, (1..=6).collect::<Vec<_>>());
        let again = db
            .with(|c| Ok(publish(c, &caps(), tz(), RUN, now())))
            .unwrap()
            .unwrap_err();
        assert_eq!(
            again.to_string(),
            format!("This run already published digest '2026-09-17-{RUN}'.")
        );
    }

    #[test]
    fn publish_id_uses_local_date() {
        let db = seeded();
        stage_all(&db, 24, 6);
        let late_utc = Utc.with_ymd_and_hms(2026, 9, 17, 23, 30, 0).unwrap();
        let published = db
            .with(|c| Ok(publish(c, &caps(), tz(), RUN, late_utc)))
            .unwrap()
            .unwrap();
        assert_eq!(published.date, "2026-09-18");
        assert!(published.digest_id.starts_with("2026-09-18-"));
        let row = db
            .with(|c| repo::get_digest_by_run(c, RUN))
            .unwrap()
            .unwrap();
        assert_eq!(row.published_at, "2026-09-17T23:30:00.000Z");
        assert_eq!(row.for_you_count, 24);
    }
}
