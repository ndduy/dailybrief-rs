//! Per-run staging (`SPEC.md` §4 `read_item` and `select`): the read cap, read-before-select,
//! word caps, topic and reason validation, and the per-source / per-topic / per-section caps
//! checked incrementally, in the spec's order. Every rejection is one sentence naming the value
//! and the cap, and the wording is snapshot-tested because the prompt quotes it.

use chrono::{DateTime, Utc};

use super::extract::count_words;
use super::time::days_ago_iso;
use crate::config::Caps;
use crate::db::repo::{self, Section, Selection};
use crate::db::{Connection, DbError};

pub const EXPLORE_REASONS: [&str; 5] = [
    "new_to_me",
    "adjacent_field",
    "contrarian",
    "deep_dive",
    "emerging",
];
pub const SUMMARY_WORDS: usize = 80;
pub const WHY_WORDS: usize = 25;

#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    #[error("No item with id '{id}'. Use list_candidates or search_items.")]
    UnknownItem { id: String },
    #[error("Read cap of {cap} reached for this run. Select from what you have read.")]
    ReadCap { cap: u32 },
    #[error("Read item '{id}' with read_item before selecting it.")]
    Unread { id: String },
    #[error("summary is required.")]
    EmptySummary,
    #[error("whyItMatters is required.")]
    EmptyWhy,
    #[error("summary is {words} words; the cap is {cap}.")]
    SummaryTooLong { words: usize, cap: usize },
    #[error("whyItMatters is {words} words; the cap is {cap}.")]
    WhyTooLong { words: usize, cap: usize },
    #[error("Unknown topic '{topic}'. Known topics: {known}.")]
    UnknownTopic { topic: String, known: String },
    #[error("beyond_radar needs a reason: {reasons}.")]
    MissingReason { reasons: String },
    #[error("Unknown reason '{reason}'. Reasons: {reasons}.")]
    UnknownReason { reason: String, reasons: String },
    #[error("Item '{id}' was shown in the last {days} days.")]
    ShownRecently { id: String, days: u32 },
    #[error(
        "Source '{source_title}' already has {have} selected items; the cap is {cap} per source."
    )]
    SourceCap {
        source_title: String,
        have: usize,
        cap: u32,
    },
    #[error("Topic '{topic}' already has {have} selected items; the cap is {cap} per topic.")]
    TopicCap {
        topic: String,
        have: usize,
        cap: u32,
    },
    #[error("{section} already has {have} items; the cap is {cap}.")]
    SectionCap {
        section: &'static str,
        have: usize,
        cap: u32,
    },
    #[error("database: {0}")]
    Db(#[from] DbError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOutcome {
    pub id: String,
    pub title: String,
    pub source: String,
    /// The first `caps.read_text_chars` characters.
    pub text: String,
    pub word_count: i64,
    pub truncated: bool,
}

/// Reads an item within the run's cap and records the read. Re-reading a read id is free.
pub fn read_item(
    conn: &Connection,
    caps: &Caps,
    run_id: &str,
    id: &str,
) -> Result<ReadOutcome, StagingError> {
    let item = repo::get_item(conn, id)?
        .ok_or_else(|| StagingError::UnknownItem { id: id.to_string() })?;
    let reads = repo::list_run_reads(conn, run_id)?;
    if !reads.contains(id) && reads.len() >= caps.reads as usize {
        return Err(StagingError::ReadCap { cap: caps.reads });
    }
    repo::mark_run_read(conn, run_id, id)?;
    let source =
        repo::get_source(conn, &item.source_id)?.map_or(item.source_id.clone(), |s| s.title);
    let text: String = item
        .text
        .chars()
        .take(caps.read_text_chars as usize)
        .collect();
    let truncated = text.chars().count() < item.text.chars().count();
    Ok(ReadOutcome {
        id: item.id,
        title: item.title,
        source,
        text,
        word_count: item.word_count,
        truncated,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectInput {
    /// `section: "none"` drops the staged row; never an error.
    Unstage { item_id: String },
    Stage {
        section: Section,
        item_id: String,
        summary: String,
        why_it_matters: String,
        reason: Option<String>,
        topic: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StagedCounts {
    pub for_you: usize,
    pub beyond_radar: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOutcome {
    pub replaced: bool,
    pub staged: StagedCounts,
}

pub fn staged_counts(conn: &Connection, run_id: &str) -> Result<StagedCounts, DbError> {
    let rows = repo::list_selections(conn, run_id)?;
    Ok(StagedCounts {
        for_you: rows.iter().filter(|r| r.section == Section::ForYou).count(),
        beyond_radar: rows
            .iter()
            .filter(|r| r.section == Section::BeyondRadar)
            .count(),
    })
}

fn reasons_list() -> String {
    EXPLORE_REASONS.join(", ")
}

/// Stages one item, applying the rejections of `SPEC.md` §4 in order.
pub fn stage_selection(
    conn: &Connection,
    caps: &Caps,
    run_id: &str,
    now: DateTime<Utc>,
    input: SelectInput,
) -> Result<SelectOutcome, StagingError> {
    let (section, item_id, summary, why, reason, topic_name) = match input {
        SelectInput::Unstage { item_id } => {
            repo::delete_selection(conn, run_id, &item_id)?;
            return Ok(SelectOutcome {
                replaced: false,
                staged: staged_counts(conn, run_id)?,
            });
        }
        SelectInput::Stage {
            section,
            item_id,
            summary,
            why_it_matters,
            reason,
            topic,
        } => (section, item_id, summary, why_it_matters, reason, topic),
    };
    let item = repo::get_item(conn, &item_id)?.ok_or_else(|| StagingError::UnknownItem {
        id: item_id.clone(),
    })?;
    if !repo::list_run_reads(conn, run_id)?.contains(&item.id) {
        return Err(StagingError::Unread { id: item.id });
    }
    let summary = summary.trim().to_string();
    let why = why.trim().to_string();
    if summary.is_empty() {
        return Err(StagingError::EmptySummary);
    }
    if why.is_empty() {
        return Err(StagingError::EmptyWhy);
    }
    let words = count_words(&summary);
    if words > SUMMARY_WORDS {
        return Err(StagingError::SummaryTooLong {
            words,
            cap: SUMMARY_WORDS,
        });
    }
    let words = count_words(&why);
    if words > WHY_WORDS {
        return Err(StagingError::WhyTooLong {
            words,
            cap: WHY_WORDS,
        });
    }
    let topics = repo::list_topics(conn)?;
    let wanted = topic_name.trim().to_lowercase();
    let Some(topic) = topics.iter().find(|t| t.name.to_lowercase() == wanted) else {
        return Err(StagingError::UnknownTopic {
            topic: topic_name.trim().to_string(),
            known: topics
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        });
    };
    let reason = match (
        section,
        reason.as_deref().map(str::trim).filter(|r| !r.is_empty()),
    ) {
        (Section::BeyondRadar, None) => {
            return Err(StagingError::MissingReason {
                reasons: reasons_list(),
            });
        }
        (_, Some(r)) if !EXPLORE_REASONS.contains(&r) => {
            return Err(StagingError::UnknownReason {
                reason: r.to_string(),
                reasons: reasons_list(),
            });
        }
        (_, r) => r.map(str::to_string),
    };
    if repo::shown_item_ids(conn, &days_ago_iso(now, caps.shown_days))?.contains(&item.id) {
        return Err(StagingError::ShownRecently {
            id: item.id,
            days: caps.shown_days,
        });
    }
    let existing = repo::list_selections(conn, run_id)?;
    let previous = existing.iter().find(|s| s.item_id == item.id).cloned();
    let others: Vec<&Selection> = existing.iter().filter(|s| s.item_id != item.id).collect();
    let mut same_source = 0;
    for s in &others {
        if repo::get_item(conn, &s.item_id)?.is_some_and(|i| i.source_id == item.source_id) {
            same_source += 1;
        }
    }
    if same_source >= caps.per_source as usize {
        let source =
            repo::get_source(conn, &item.source_id)?.map_or(item.source_id.clone(), |s| s.title);
        return Err(StagingError::SourceCap {
            source_title: source,
            have: same_source,
            cap: caps.per_source,
        });
    }
    let same_topic = others
        .iter()
        .filter(|s| s.topic.to_lowercase() == wanted)
        .count();
    if same_topic >= caps.per_topic as usize {
        return Err(StagingError::TopicCap {
            topic: topic.name.clone(),
            have: same_topic,
            cap: caps.per_topic,
        });
    }
    let section_cap = match section {
        Section::ForYou => caps.for_you,
        Section::BeyondRadar => caps.beyond_radar,
    };
    let same_section = others.iter().filter(|s| s.section == section).count();
    if same_section >= section_cap as usize {
        return Err(StagingError::SectionCap {
            section: section.as_str(),
            have: same_section,
            cap: section_cap,
        });
    }
    let position = match &previous {
        Some(p) if p.section == section => p.position,
        _ => same_section as i64 + 1,
    };
    repo::upsert_selection(
        conn,
        run_id,
        &Selection {
            item_id: item.id,
            section,
            position,
            summary,
            why_it_matters: why,
            reason,
            topic: topic.name.clone(),
        },
    )?;
    Ok(SelectOutcome {
        replaced: previous.is_some(),
        staged: staged_counts(conn, run_id)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Env, load_config};
    use crate::core::testutil::{item, shown, source, topic};
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

    fn v() -> Option<Vec<f32>> {
        Some(vec![0.1; DIMENSIONS])
    }

    /// One source, two topics, `n` read items `i00..` from source `s` (plus any extras).
    fn db_with(n: usize) -> Db {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            source(c, "other")?;
            topic(c, "rust", "Rust")?;
            topic(c, "ai", "AI Agents")?;
            for i in 0..n {
                let id = format!("i{i:02}");
                item(c, &id, "s", v(), RECENT)?;
                repo::mark_run_read(c, RUN, &id)?;
            }
            Ok(())
        })
        .unwrap();
        db
    }

    fn stage(id: &str, section: Section, topic: &str, reason: Option<&str>) -> SelectInput {
        SelectInput::Stage {
            section,
            item_id: id.into(),
            summary: "A fine summary.".into(),
            why_it_matters: "Because.".into(),
            reason: reason.map(str::to_string),
            topic: topic.into(),
        }
    }

    fn err(db: &Db, input: SelectInput) -> StagingError {
        db.with(|c| Ok(stage_selection(c, &caps(), RUN, now(), input)))
            .unwrap()
            .unwrap_err()
    }

    fn ok(db: &Db, input: SelectInput) -> SelectOutcome {
        db.with(|c| Ok(stage_selection(c, &caps(), RUN, now(), input)))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn read_item_caps_distinct_ids_at_45_and_reread_is_free() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            for i in 0..46 {
                item(c, &format!("i{i:02}"), "s", v(), RECENT)?;
            }
            Ok(())
        })
        .unwrap();
        for i in 0..45 {
            db.with(|c| Ok(read_item(c, &caps(), RUN, &format!("i{i:02}"))))
                .unwrap()
                .unwrap();
        }
        let e = db
            .with(|c| Ok(read_item(c, &caps(), RUN, "i45")))
            .unwrap()
            .unwrap_err();
        assert_eq!(
            e.to_string(),
            "Read cap of 45 reached for this run. Select from what you have read."
        );
        let again = db
            .with(|c| Ok(read_item(c, &caps(), RUN, "i00")))
            .unwrap()
            .unwrap();
        assert_eq!(again.id, "i00");
        assert_eq!(again.source, "S");
    }

    #[test]
    fn read_item_truncates_by_chars_and_reports_unknown_ids() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            let mut long = crate::core::testutil::new_item("long", "s", v(), RECENT);
            long.text = "ố".repeat(5001);
            repo::insert_item(c, &long)
        })
        .unwrap();
        let r = db
            .with(|c| Ok(read_item(c, &caps(), RUN, "long")))
            .unwrap()
            .unwrap();
        assert!(r.truncated);
        assert_eq!(r.text.chars().count(), 5000);
        let e = db
            .with(|c| Ok(read_item(c, &caps(), RUN, "nope")))
            .unwrap()
            .unwrap_err();
        assert!(matches!(e, StagingError::UnknownItem { .. }));
    }

    #[test]
    fn select_rejects_unknown_item() {
        let db = db_with(1);
        assert!(matches!(
            err(&db, stage("nope", Section::ForYou, "Rust", None)),
            StagingError::UnknownItem { .. }
        ));
    }

    #[test]
    fn select_rejects_unread_item() {
        let db = db_with(1);
        db.with(|c| item(c, "unread", "s", v(), RECENT)).unwrap();
        assert!(matches!(
            err(&db, stage("unread", Section::ForYou, "Rust", None)),
            StagingError::Unread { .. }
        ));
    }

    #[test]
    fn select_rejects_empty_summary_and_why() {
        let db = db_with(1);
        let mut s = stage("i00", Section::ForYou, "Rust", None);
        if let SelectInput::Stage { summary, .. } = &mut s {
            *summary = "   ".into();
        }
        assert!(matches!(err(&db, s), StagingError::EmptySummary));
        let mut w = stage("i00", Section::ForYou, "Rust", None);
        if let SelectInput::Stage { why_it_matters, .. } = &mut w {
            *why_it_matters = String::new();
        }
        assert!(matches!(err(&db, w), StagingError::EmptyWhy));
    }

    #[test]
    fn select_rejects_over_word_caps() {
        let db = db_with(1);
        let mut s = stage("i00", Section::ForYou, "Rust", None);
        if let SelectInput::Stage { summary, .. } = &mut s {
            *summary = "từ ".repeat(81);
        }
        assert_eq!(
            err(&db, s).to_string(),
            "summary is 81 words; the cap is 80."
        );
        let mut w = stage("i00", Section::ForYou, "Rust", None);
        if let SelectInput::Stage { why_it_matters, .. } = &mut w {
            *why_it_matters = "w ".repeat(26);
        }
        assert_eq!(
            err(&db, w).to_string(),
            "whyItMatters is 26 words; the cap is 25."
        );
    }

    #[test]
    fn select_rejects_unknown_topic_listing_known_names() {
        let db = db_with(1);
        let e = err(&db, stage("i00", Section::ForYou, "Cooking", None));
        assert_eq!(
            e.to_string(),
            "Unknown topic 'Cooking'. Known topics: AI Agents, Rust."
        );
    }

    #[test]
    fn select_topic_match_is_case_insensitive() {
        let db = db_with(1);
        ok(&db, stage("i00", Section::ForYou, "  rust ", None));
        let rows = db.with(|c| repo::list_selections(c, RUN)).unwrap();
        assert_eq!(rows[0].topic, "Rust", "stored with the canonical name");
    }

    #[test]
    fn select_rejects_beyond_radar_without_or_with_bad_reason() {
        let db = db_with(1);
        let e = err(&db, stage("i00", Section::BeyondRadar, "Rust", None));
        assert!(matches!(e, StagingError::MissingReason { .. }), "{e}");
        let e = err(
            &db,
            stage("i00", Section::BeyondRadar, "Rust", Some("because")),
        );
        assert!(matches!(e, StagingError::UnknownReason { .. }), "{e}");
        let e = err(&db, stage("i00", Section::ForYou, "Rust", Some("because")));
        assert!(
            matches!(e, StagingError::UnknownReason { .. }),
            "a bad reason is rejected in for_you too"
        );
        ok(
            &db,
            stage("i00", Section::BeyondRadar, "Rust", Some("new_to_me")),
        );
    }

    #[test]
    fn select_rejects_shown_item() {
        let db = db_with(1);
        db.with(|c| shown(c, "i00", "2026-09-10T00:00:00.000Z"))
            .unwrap();
        assert!(matches!(
            err(&db, stage("i00", Section::ForYou, "Rust", None)),
            StagingError::ShownRecently { days: 14, .. }
        ));
    }

    #[test]
    fn select_rejects_fifth_from_same_source() {
        let db = db_with(5);
        for i in 0..4 {
            ok(
                &db,
                stage(&format!("i{i:02}"), Section::ForYou, "Rust", None),
            );
        }
        let e = err(&db, stage("i04", Section::ForYou, "AI Agents", None));
        assert_eq!(
            e.to_string(),
            "Source 'S' already has 4 selected items; the cap is 4 per source."
        );
    }

    #[test]
    fn select_rejects_ninth_in_same_topic() {
        let db = db_with(0);
        db.with(|c| {
            for i in 0..9 {
                let id = format!("o{i}");
                item(c, &id, if i % 2 == 0 { "s" } else { "other" }, v(), RECENT)?;
                repo::mark_run_read(c, RUN, &id)?;
            }
            Ok(())
        })
        .unwrap();
        for i in 0..8 {
            ok(&db, stage(&format!("o{i}"), Section::ForYou, "Rust", None));
        }
        // o8 is from source "s" which has 4 already → use the topic cap on a beyond_radar slot
        // from a fresh source instead.
        db.with(|c| {
            source(c, "third")?;
            item(c, "t1", "third", v(), RECENT)?;
            repo::mark_run_read(c, RUN, "t1")
        })
        .unwrap();
        let e = err(&db, stage("t1", Section::ForYou, "rust", None));
        assert_eq!(
            e.to_string(),
            "Topic 'Rust' already has 8 selected items; the cap is 8 per topic."
        );
    }

    #[test]
    fn select_rejects_section_caps() {
        let db = db_with(0);
        db.with(|c| {
            for i in 0..31 {
                let src = format!("src{}", i / 4);
                source(c, &src)?;
                let id = format!("x{i:02}");
                item(c, &id, &src, v(), RECENT)?;
                repo::mark_run_read(c, RUN, &id)?;
            }
            for t in 0..4 {
                topic(c, &format!("t{t}"), &format!("Topic {t}"))?;
            }
            Ok(())
        })
        .unwrap();
        for i in 0..24 {
            ok(
                &db,
                stage(
                    &format!("x{i:02}"),
                    Section::ForYou,
                    &format!("Topic {}", i / 8),
                    None,
                ),
            );
        }
        let e = err(&db, stage("x24", Section::ForYou, "Topic 3", None));
        assert_eq!(
            e.to_string(),
            "for_you already has 24 items; the cap is 24."
        );
        for i in 24..30 {
            ok(
                &db,
                stage(
                    &format!("x{i:02}"),
                    Section::BeyondRadar,
                    "Topic 3",
                    Some("emerging"),
                ),
            );
        }
        let e = err(
            &db,
            stage("x30", Section::BeyondRadar, "Topic 3", Some("emerging")),
        );
        assert_eq!(
            e.to_string(),
            "beyond_radar already has 6 items; the cap is 6."
        );
    }

    #[test]
    fn select_none_unstages_and_replace_keeps_position() {
        let db = db_with(3);
        ok(&db, stage("i00", Section::ForYou, "Rust", None));
        let second = ok(&db, stage("i01", Section::ForYou, "Rust", None));
        assert_eq!(
            second.staged,
            StagedCounts {
                for_you: 2,
                beyond_radar: 0
            }
        );
        let replaced = ok(&db, stage("i00", Section::ForYou, "AI Agents", None));
        assert!(replaced.replaced);
        let rows = db.with(|c| repo::list_selections(c, RUN)).unwrap();
        let i00 = rows.iter().find(|r| r.item_id == "i00").unwrap();
        assert_eq!(i00.position, 1, "same section keeps its position");
        assert_eq!(i00.topic, "AI Agents");
        let moved = ok(
            &db,
            stage("i00", Section::BeyondRadar, "Rust", Some("deep_dive")),
        );
        assert!(moved.replaced);
        assert_eq!(
            moved.staged,
            StagedCounts {
                for_you: 1,
                beyond_radar: 1
            }
        );
        let gone = ok(
            &db,
            SelectInput::Unstage {
                item_id: "i00".into(),
            },
        );
        assert!(!gone.replaced);
        assert_eq!(
            gone.staged,
            StagedCounts {
                for_you: 1,
                beyond_radar: 0
            }
        );
        ok(
            &db,
            SelectInput::Unstage {
                item_id: "never-staged".into(),
            },
        );
    }

    #[test]
    fn staging_error_messages_snapshot() {
        let all = vec![
            StagingError::UnknownItem { id: "x".into() },
            StagingError::ReadCap { cap: 45 },
            StagingError::Unread { id: "x".into() },
            StagingError::EmptySummary,
            StagingError::EmptyWhy,
            StagingError::SummaryTooLong { words: 81, cap: 80 },
            StagingError::WhyTooLong { words: 26, cap: 25 },
            StagingError::UnknownTopic {
                topic: "Cooking".into(),
                known: "AI Agents, Rust".into(),
            },
            StagingError::MissingReason {
                reasons: reasons_list(),
            },
            StagingError::UnknownReason {
                reason: "because".into(),
                reasons: reasons_list(),
            },
            StagingError::ShownRecently {
                id: "x".into(),
                days: 14,
            },
            StagingError::SourceCap {
                source_title: "S".into(),
                have: 4,
                cap: 4,
            },
            StagingError::TopicCap {
                topic: "Rust".into(),
                have: 8,
                cap: 8,
            },
            StagingError::SectionCap {
                section: "for_you",
                have: 24,
                cap: 24,
            },
        ];
        let rendered: Vec<String> = all.iter().map(ToString::to_string).collect();
        insta::assert_debug_snapshot!(rendered);
    }
}
