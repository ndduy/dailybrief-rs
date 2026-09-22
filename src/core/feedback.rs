//! Ratings and proposals (`spec/m3.md`, ADR 0016, ADR 0018). A rating is the reader's word and
//! changes nothing else; a proposal is the only way an agent asks for a change, and [`apply`]
//! (called by the approval page alone) is the only writer of topic and source state. All SQL
//! stays in `db::repo`; this module holds the types, the bounds and the one transaction.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{TopicOrigin, is_valid_id};
use crate::core::time::to_iso;
use crate::db::{Connection, DbError, repo};

/// Bounds shared with the prompt: a proposal outside them is refused before it is stored.
pub const MIN_WEIGHT: f64 = 0.1;
pub const MAX_WEIGHT: f64 = 5.0;
pub const MAX_DESCRIPTION_CHARS: usize = 200;
pub const MAX_NAME_CHARS: usize = 80;
pub const MAX_NOTES_CHARS: usize = 500;
pub const MAX_SUMMARY_CHARS: usize = 500;

#[derive(Debug, thiserror::Error)]
pub enum FeedbackError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("{0}")]
    Invalid(String),
    #[error("proposal '{0}' does not exist")]
    UnknownProposal(String),
    #[error("proposal '{id}' is already {status}")]
    NotPending { id: String, status: ProposalStatus },
    #[error("{0}")]
    MissingTarget(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sign {
    Up,
    Down,
}

impl Sign {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }
}

/// The fixed reason set of `SPEC.md` §6, four per sign; no free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    NewToMe,
    DeepActionable,
    RelevantToCurrentWork,
    GoodSource,
    AlreadyKnow,
    OffTopic,
    LowQuality,
    TooShallow,
}

impl Reason {
    pub const ALL: [Reason; 8] = [
        Reason::NewToMe,
        Reason::DeepActionable,
        Reason::RelevantToCurrentWork,
        Reason::GoodSource,
        Reason::AlreadyKnow,
        Reason::OffTopic,
        Reason::LowQuality,
        Reason::TooShallow,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NewToMe => "new_to_me",
            Self::DeepActionable => "deep_actionable",
            Self::RelevantToCurrentWork => "relevant_to_current_work",
            Self::GoodSource => "good_source",
            Self::AlreadyKnow => "already_know",
            Self::OffTopic => "off_topic",
            Self::LowQuality => "low_quality",
            Self::TooShallow => "too_shallow",
        }
    }

    /// The text on the button.
    pub fn label(self) -> &'static str {
        match self {
            Self::NewToMe => "New to me",
            Self::DeepActionable => "Deep / actionable",
            Self::RelevantToCurrentWork => "Relevant to current work",
            Self::GoodSource => "Good source",
            Self::AlreadyKnow => "Already know this",
            Self::OffTopic => "Off-topic",
            Self::LowQuality => "Low quality / clickbait",
            Self::TooShallow => "Too shallow",
        }
    }

    pub fn sign(self) -> Sign {
        match self {
            Self::NewToMe
            | Self::DeepActionable
            | Self::RelevantToCurrentWork
            | Self::GoodSource => Sign::Up,
            _ => Sign::Down,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }

    pub fn for_sign(sign: Sign) -> Vec<Reason> {
        Self::ALL.into_iter().filter(|r| r.sign() == sign).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Approved,
    Rejected,
}

impl ProposalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

impl std::fmt::Display for ProposalStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What the Curator may propose (`SPEC.md` §5). Payloads are typed; unknown kinds never parse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum ProposalChange {
    TopicWeight {
        #[serde(rename = "topicId")]
        topic_id: String,
        weight: f64,
    },
    AddTopic {
        name: String,
        #[serde(default)]
        description: String,
        weight: f64,
    },
    DisableSource {
        #[serde(rename = "sourceId")]
        source_id: String,
    },
    AddSource {
        url: String,
        title: String,
    },
    PromoteExploreTopic {
        #[serde(rename = "topicId")]
        topic_id: String,
    },
}

impl ProposalChange {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::TopicWeight { .. } => "topic_weight",
            Self::AddTopic { .. } => "add_topic",
            Self::DisableSource { .. } => "disable_source",
            Self::AddSource { .. } => "add_source",
            Self::PromoteExploreTopic { .. } => "promote_explore_topic",
        }
    }

    /// The bounds the prompt is told about; the message is one sentence it can quote.
    pub fn validate(&self) -> Result<(), FeedbackError> {
        let weight_ok = |w: f64| (MIN_WEIGHT..=MAX_WEIGHT).contains(&w) && w.is_finite();
        match self {
            Self::TopicWeight { topic_id, weight } => {
                if !is_valid_id(topic_id) {
                    return Err(FeedbackError::Invalid(format!(
                        "topicId '{topic_id}' is not a topic id."
                    )));
                }
                if !weight_ok(*weight) {
                    return Err(FeedbackError::Invalid(format!(
                        "weight must be within {MIN_WEIGHT}..={MAX_WEIGHT}, got {weight}."
                    )));
                }
            }
            Self::AddTopic {
                name,
                description,
                weight,
            } => {
                let n = name.trim().chars().count();
                if n == 0 || n > MAX_NAME_CHARS {
                    return Err(FeedbackError::Invalid(format!(
                        "name must be 1..={MAX_NAME_CHARS} characters."
                    )));
                }
                if description.chars().count() > MAX_DESCRIPTION_CHARS {
                    return Err(FeedbackError::Invalid(format!(
                        "description is {} characters; the cap is {MAX_DESCRIPTION_CHARS}.",
                        description.chars().count()
                    )));
                }
                if !weight_ok(*weight) {
                    return Err(FeedbackError::Invalid(format!(
                        "weight must be within {MIN_WEIGHT}..={MAX_WEIGHT}, got {weight}."
                    )));
                }
            }
            Self::DisableSource { source_id } => {
                if !is_valid_id(source_id) {
                    return Err(FeedbackError::Invalid(format!(
                        "sourceId '{source_id}' is not a source id."
                    )));
                }
            }
            Self::AddSource { url, title } => {
                let parsed = url::Url::parse(url).ok();
                if !parsed.is_some_and(|u| matches!(u.scheme(), "http" | "https")) {
                    return Err(FeedbackError::Invalid("url must be an http(s) URL.".into()));
                }
                let n = title.trim().chars().count();
                if n == 0 || n > MAX_NAME_CHARS {
                    return Err(FeedbackError::Invalid(format!(
                        "title must be 1..={MAX_NAME_CHARS} characters."
                    )));
                }
            }
            Self::PromoteExploreTopic { topic_id } => {
                if !is_valid_id(topic_id) {
                    return Err(FeedbackError::Invalid(format!(
                        "topicId '{topic_id}' is not a topic id."
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Why the Curator proposes it: fixed fields the page renders as a list, plus a short note.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub summary: String,
    #[serde(default)]
    pub rating_ids: Vec<i64>,
    #[serde(default)]
    pub read_count: u32,
    #[serde(default)]
    pub feed_issue_ids: Vec<i64>,
    #[serde(default)]
    pub notes: String,
}

impl Evidence {
    pub fn validate(&self) -> Result<(), FeedbackError> {
        let s = self.summary.trim().chars().count();
        if s == 0 || s > MAX_SUMMARY_CHARS {
            return Err(FeedbackError::Invalid(format!(
                "evidence.summary must be 1..={MAX_SUMMARY_CHARS} characters."
            )));
        }
        if self.notes.chars().count() > MAX_NOTES_CHARS {
            return Err(FeedbackError::Invalid(format!(
                "evidence.notes is {} characters; the cap is {MAX_NOTES_CHARS}.",
                self.notes.chars().count()
            )));
        }
        Ok(())
    }
}

/// A stored proposal with its typed change and evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    pub id: String,
    pub run_id: Option<String>,
    pub change: ProposalChange,
    pub evidence: Evidence,
    pub status: ProposalStatus,
    pub created_at: String,
    pub decided_at: Option<String>,
    pub applied: Option<Value>,
}

impl Proposal {
    /// Decodes a repo row; a row whose JSON no longer parses is a `Corrupt` error, not a panic.
    pub fn from_row(row: repo::ProposalRow) -> Result<Self, DbError> {
        let change: ProposalChange = serde_json::from_str(&row.payload_json).map_err(|e| {
            DbError::Corrupt(format!("proposal {}: payload does not parse: {e}", row.id))
        })?;
        if change.kind() != row.kind {
            return Err(DbError::Corrupt(format!(
                "proposal {}: kind column '{}' does not match payload kind '{}'",
                row.id,
                row.kind,
                change.kind()
            )));
        }
        let evidence: Evidence = serde_json::from_str(&row.evidence_json).map_err(|e| {
            DbError::Corrupt(format!("proposal {}: evidence does not parse: {e}", row.id))
        })?;
        let status = ProposalStatus::parse(&row.status).ok_or_else(|| {
            DbError::Corrupt(format!("proposal {}: status '{}'", row.id, row.status))
        })?;
        let applied = match row.applied_json {
            Some(text) => Some(serde_json::from_str(&text).map_err(|e| {
                DbError::Corrupt(format!("proposal {}: applied does not parse: {e}", row.id))
            })?),
            None => None,
        };
        Ok(Self {
            id: row.id,
            run_id: row.run_id,
            change,
            evidence,
            status,
            created_at: row.created_at,
            decided_at: row.decided_at,
            applied,
        })
    }
}

/// `p-YYYY-MM-DD-<8 hex>`, like run ids: sortable, unique, safe in a URL.
pub fn new_proposal_id(now: DateTime<Utc>) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("p-{}-{}", now.format("%Y-%m-%d"), &hex[..8])
}

/// Validates and stores a proposal; returns its id. This is all `propose_change` may do.
pub fn propose(
    conn: &Connection,
    run_id: Option<&str>,
    change: &ProposalChange,
    evidence: &Evidence,
    now: DateTime<Utc>,
) -> Result<String, FeedbackError> {
    change.validate()?;
    evidence.validate()?;
    let id = new_proposal_id(now);
    let payload_json = serde_json::to_string(change)
        .map_err(|e| FeedbackError::Invalid(format!("payload does not serialise: {e}")))?;
    let evidence_json = serde_json::to_string(evidence)
        .map_err(|e| FeedbackError::Invalid(format!("evidence does not serialise: {e}")))?;
    repo::insert_proposal(
        conn,
        &repo::NewProposal {
            id: id.clone(),
            run_id: run_id.map(str::to_string),
            kind: change.kind().to_string(),
            payload_json,
            evidence_json,
            created_at: to_iso(now),
        },
    )?;
    Ok(id)
}

/// `[a-z0-9][a-z0-9_-]{0,63}` from a display name, for topics and sources created by proposals.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_end_matches('-');
    let s: String = trimmed.chars().take(64).collect();
    if s.is_empty() { "topic".to_string() } else { s }
}

/// Loads a pending proposal and applies its change in one transaction: the target rows and
/// the proposal's own row change together or not at all. The only writer of topic and source
/// state outside `topics.toml` mirroring (`only_feedback_apply_writes_topics_and_sources`).
pub fn apply(conn: &Connection, id: &str, now: DateTime<Utc>) -> Result<Proposal, FeedbackError> {
    repo::in_transaction(conn, |tx| apply_in(tx, id, now))?;
    let row = repo::get_proposal(conn, id)?
        .ok_or_else(|| FeedbackError::UnknownProposal(id.to_string()))?;
    Ok(Proposal::from_row(row)?)
}

fn apply_in(tx: &Connection, id: &str, now: DateTime<Utc>) -> Result<(), FeedbackError> {
    let row = repo::get_proposal(tx, id)?
        .ok_or_else(|| FeedbackError::UnknownProposal(id.to_string()))?;
    let proposal = Proposal::from_row(row)?;
    if proposal.status != ProposalStatus::Pending {
        return Err(FeedbackError::NotPending {
            id: id.to_string(),
            status: proposal.status,
        });
    }
    let applied = match &proposal.change {
        ProposalChange::TopicWeight { topic_id, weight } => {
            let before = repo::get_topic(tx, topic_id)?.ok_or_else(|| {
                FeedbackError::MissingTarget(format!("topic '{topic_id}' does not exist"))
            })?;
            repo::set_topic_weight(tx, topic_id, *weight)?;
            json!({ "topicId": topic_id, "weightBefore": before.weight, "weightAfter": weight })
        }
        ProposalChange::AddTopic {
            name,
            description,
            weight,
        } => {
            let topic_id = slug(name);
            if repo::get_topic(tx, &topic_id)?.is_some() {
                return Err(FeedbackError::MissingTarget(format!(
                    "topic '{topic_id}' already exists"
                )));
            }
            repo::insert_topic_from_proposal(tx, &topic_id, name.trim(), description, *weight)?;
            json!({ "topicId": topic_id, "origin": TopicOrigin::Curator.as_str(), "weight": weight })
        }
        ProposalChange::DisableSource { source_id } => {
            let before = repo::get_source(tx, source_id)?.ok_or_else(|| {
                FeedbackError::MissingTarget(format!("source '{source_id}' does not exist"))
            })?;
            if !before.enabled {
                return Err(FeedbackError::MissingTarget(format!(
                    "source '{source_id}' is already disabled"
                )));
            }
            repo::set_source_enabled(tx, source_id, false)?;
            json!({ "sourceId": source_id, "enabled": false })
        }
        ProposalChange::AddSource { url, title } => {
            let source_id = slug(title);
            if repo::get_source(tx, &source_id)?.is_some() {
                return Err(FeedbackError::MissingTarget(format!(
                    "source '{source_id}' already exists"
                )));
            }
            repo::insert_source_from_proposal(tx, &source_id, url, title.trim())?;
            json!({ "sourceId": source_id, "url": url })
        }
        ProposalChange::PromoteExploreTopic { topic_id } => {
            let before = repo::get_topic(tx, topic_id)?.ok_or_else(|| {
                FeedbackError::MissingTarget(format!("topic '{topic_id}' does not exist"))
            })?;
            let after = (before.weight + 0.5).min(MAX_WEIGHT);
            repo::set_topic_origin(tx, topic_id, TopicOrigin::ExplorePromoted)?;
            repo::set_topic_weight(tx, topic_id, after)?;
            json!({ "topicId": topic_id, "origin": TopicOrigin::ExplorePromoted.as_str(), "weightBefore": before.weight, "weightAfter": after })
        }
    };
    let applied_json = serde_json::to_string(&applied)
        .map_err(|e| FeedbackError::Invalid(format!("applied does not serialise: {e}")))?;
    let changed = repo::decide_proposal(
        tx,
        id,
        ProposalStatus::Approved,
        &to_iso(now),
        Some(&applied_json),
    )?;
    if changed != 1 {
        return Err(FeedbackError::NotPending {
            id: id.to_string(),
            status: proposal.status,
        });
    }
    Ok(())
}

/// Marks a pending proposal rejected; nothing else changes.
pub fn reject(conn: &Connection, id: &str, now: DateTime<Utc>) -> Result<Proposal, FeedbackError> {
    let row = repo::get_proposal(conn, id)?
        .ok_or_else(|| FeedbackError::UnknownProposal(id.to_string()))?;
    let proposal = Proposal::from_row(row)?;
    if proposal.status != ProposalStatus::Pending {
        return Err(FeedbackError::NotPending {
            id: id.to_string(),
            status: proposal.status,
        });
    }
    repo::decide_proposal(conn, id, ProposalStatus::Rejected, &to_iso(now), None)?;
    let row = repo::get_proposal(conn, id)?
        .ok_or_else(|| FeedbackError::UnknownProposal(id.to_string()))?;
    Ok(Proposal::from_row(row)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Feed, Topic};
    use crate::db::Db;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 22, 9, 0, 0).unwrap()
    }

    fn seeded() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            repo::upsert_topic(
                c,
                &Topic {
                    id: "rust".into(),
                    name: "Rust".into(),
                    description: "the language".into(),
                    weight: 1.0,
                    origin: TopicOrigin::Seed,
                },
            )?;
            repo::upsert_source(
                c,
                &Feed {
                    id: "blog".into(),
                    url: "https://blog.example/rss".into(),
                    title: "Blog".into(),
                    weight: 1.0,
                    enabled: true,
                },
            )?;
            Ok(())
        })
        .unwrap();
        db
    }

    fn evidence() -> Evidence {
        Evidence {
            summary: "three off-topic ratings in a week".into(),
            rating_ids: vec![1, 2, 3],
            read_count: 0,
            feed_issue_ids: vec![],
            notes: String::new(),
        }
    }

    fn counts(db: &Db) -> Vec<(&'static str, i64)> {
        db.with(|c| repo::row_counts(c)).unwrap()
    }

    #[test]
    fn reasons_round_trip_and_nothing_else_parses() {
        for r in Reason::ALL {
            assert_eq!(Reason::parse(r.as_str()), Some(r));
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(json, format!("\"{}\"", r.as_str()));
            assert!(!r.label().is_empty());
        }
        assert_eq!(Reason::for_sign(Sign::Up).len(), 4);
        assert_eq!(Reason::for_sign(Sign::Down).len(), 4);
        assert_eq!(Reason::parse("brilliant"), None);
        assert!(serde_json::from_str::<Reason>("\"brilliant\"").is_err());
        assert_eq!(Sign::parse("up"), Some(Sign::Up));
        assert_eq!(Sign::parse("meh"), None);
    }

    #[test]
    fn proposal_change_bounds_are_refused() {
        let bad = [
            ProposalChange::TopicWeight {
                topic_id: "rust".into(),
                weight: 0.0,
            },
            ProposalChange::TopicWeight {
                topic_id: "rust".into(),
                weight: 6.0,
            },
            ProposalChange::TopicWeight {
                topic_id: "Not An Id".into(),
                weight: 1.0,
            },
            ProposalChange::AddTopic {
                name: "x".into(),
                description: "d".repeat(201),
                weight: 1.0,
            },
            ProposalChange::AddTopic {
                name: String::new(),
                description: String::new(),
                weight: 1.0,
            },
            ProposalChange::AddSource {
                url: "ftp://x".into(),
                title: "T".into(),
            },
            ProposalChange::AddSource {
                url: "https://x.example/rss".into(),
                title: " ".into(),
            },
            ProposalChange::DisableSource {
                source_id: "../x".into(),
            },
        ];
        for b in bad {
            assert!(
                matches!(b.validate(), Err(FeedbackError::Invalid(_))),
                "{b:?}"
            );
        }
        assert!(
            ProposalChange::TopicWeight {
                topic_id: "rust".into(),
                weight: 2.5
            }
            .validate()
            .is_ok()
        );
        assert!(
            serde_json::from_str::<ProposalChange>(r#"{"kind":"rename_topic","payload":{}}"#)
                .is_err()
        );
        let round: ProposalChange = serde_json::from_str(
            r#"{"kind":"topic_weight","payload":{"topicId":"rust","weight":2}}"#,
        )
        .unwrap();
        assert_eq!(
            round,
            ProposalChange::TopicWeight {
                topic_id: "rust".into(),
                weight: 2.0
            }
        );
        let mut e = evidence();
        e.notes = "n".repeat(501);
        assert!(matches!(e.validate(), Err(FeedbackError::Invalid(_))));
    }

    #[test]
    fn propose_stores_one_pending_row() {
        let db = seeded();
        db.with(|c| {
            repo::insert_run(
                c,
                &repo::NewRun {
                    id: "run-x".into(),
                    kind: repo::RunKind::Manual,
                    harness: "fake".into(),
                    attempt: 1,
                    started_at: to_iso(now()),
                    transcript_path: None,
                },
            )
        })
        .unwrap();
        let before = counts(&db);
        let id = db
            .with(|c| {
                Ok(propose(
                    c,
                    Some("run-x"),
                    &ProposalChange::TopicWeight {
                        topic_id: "rust".into(),
                        weight: 2.0,
                    },
                    &evidence(),
                    now(),
                )
                .unwrap())
            })
            .unwrap();
        assert!(id.starts_with("p-2026-09-22-"));
        let after = counts(&db);
        let diff: Vec<_> = before.iter().zip(&after).filter(|(a, b)| a != b).collect();
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].1, &("proposals", 1));
        let p = db
            .with(|c| Proposal::from_row(repo::get_proposal(c, &id)?.unwrap()))
            .unwrap();
        assert_eq!(p.status, ProposalStatus::Pending);
        assert_eq!(p.run_id.as_deref(), Some("run-x"));
        assert_eq!(p.evidence.rating_ids, vec![1, 2, 3]);
        let err = db
            .with(|c| {
                Ok(propose(
                    c,
                    Some("no-such-run"),
                    &ProposalChange::TopicWeight {
                        topic_id: "rust".into(),
                        weight: 2.0,
                    },
                    &evidence(),
                    now(),
                )
                .unwrap_err())
            })
            .unwrap();
        assert!(err.to_string().contains("FOREIGN KEY"), "{err}");
    }

    fn apply_ok(db: &Db, change: ProposalChange) -> Proposal {
        let id = db
            .with(|c| Ok(propose(c, None, &change, &evidence(), now()).unwrap()))
            .unwrap();
        db.with(|c| Ok(apply(c, &id, now()).unwrap())).unwrap()
    }

    #[test]
    fn apply_changes_exactly_its_target_in_one_transaction() {
        let db = seeded();
        // topic_weight
        let p = apply_ok(
            &db,
            ProposalChange::TopicWeight {
                topic_id: "rust".into(),
                weight: 3.0,
            },
        );
        assert_eq!(p.status, ProposalStatus::Approved);
        assert_eq!(p.applied.as_ref().unwrap()["weightAfter"], 3.0);
        let w: f64 = db
            .with(|c| Ok(repo::get_topic(c, "rust")?.unwrap().weight))
            .unwrap();
        assert_eq!(w, 3.0);
        // add_topic
        let p = apply_ok(
            &db,
            ProposalChange::AddTopic {
                name: "Agent Engineering!".into(),
                description: "how agents get built".into(),
                weight: 1.5,
            },
        );
        assert_eq!(p.applied.as_ref().unwrap()["topicId"], "agent-engineering");
        let t = db
            .with(|c| Ok(repo::get_topic(c, "agent-engineering")?.unwrap()))
            .unwrap();
        assert_eq!((t.origin, t.weight), (TopicOrigin::Curator, 1.5));
        // promote_explore_topic
        let p = apply_ok(
            &db,
            ProposalChange::PromoteExploreTopic {
                topic_id: "agent-engineering".into(),
            },
        );
        assert_eq!(p.applied.as_ref().unwrap()["weightAfter"], 2.0);
        let t = db
            .with(|c| Ok(repo::get_topic(c, "agent-engineering")?.unwrap()))
            .unwrap();
        assert_eq!(t.origin, TopicOrigin::ExplorePromoted);
        // disable_source
        apply_ok(
            &db,
            ProposalChange::DisableSource {
                source_id: "blog".into(),
            },
        );
        let s = db
            .with(|c| Ok(repo::get_source(c, "blog")?.unwrap()))
            .unwrap();
        assert!(!s.enabled);
        // add_source
        let p = apply_ok(
            &db,
            ProposalChange::AddSource {
                url: "https://new.example/feed".into(),
                title: "New Feed".into(),
            },
        );
        assert_eq!(p.applied.as_ref().unwrap()["sourceId"], "new-feed");
        let s = db
            .with(|c| Ok(repo::get_source(c, "new-feed")?.unwrap()))
            .unwrap();
        assert!(s.enabled && s.weight == 1.0 && s.url == "https://new.example/feed");
        let after = counts(&db);
        assert_eq!(after.iter().find(|(t, _)| *t == "topics").unwrap().1, 2);
        assert_eq!(after.iter().find(|(t, _)| *t == "sources").unwrap().1, 2);
        assert_eq!(after.iter().find(|(t, _)| *t == "proposals").unwrap().1, 5);
    }

    #[test]
    fn apply_refuses_missing_target_and_non_pending() {
        let db = seeded();
        let id = db
            .with(|c| {
                Ok(propose(
                    c,
                    None,
                    &ProposalChange::TopicWeight {
                        topic_id: "nope".into(),
                        weight: 2.0,
                    },
                    &evidence(),
                    now(),
                )
                .unwrap())
            })
            .unwrap();
        let before = counts(&db);
        let err = db.with(|c| Ok(apply(c, &id, now()).unwrap_err())).unwrap();
        assert!(matches!(err, FeedbackError::MissingTarget(_)), "{err}");
        assert_eq!(
            counts(&db),
            before,
            "nothing changed, the proposal stays pending"
        );
        let p = db
            .with(|c| Proposal::from_row(repo::get_proposal(c, &id)?.unwrap()))
            .unwrap();
        assert_eq!(p.status, ProposalStatus::Pending);
        // reject, then apply is refused
        let r = db.with(|c| Ok(reject(c, &id, now()).unwrap())).unwrap();
        assert_eq!(r.status, ProposalStatus::Rejected);
        let err = db.with(|c| Ok(apply(c, &id, now()).unwrap_err())).unwrap();
        assert!(matches!(err, FeedbackError::NotPending { .. }), "{err}");
        let err = db
            .with(|c| Ok(apply(c, "p-missing", now()).unwrap_err()))
            .unwrap();
        assert!(matches!(err, FeedbackError::UnknownProposal(_)));
        // an approved one cannot be applied twice
        let ok = apply_ok(
            &db,
            ProposalChange::TopicWeight {
                topic_id: "rust".into(),
                weight: 2.0,
            },
        );
        let err = db
            .with(|c| Ok(apply(c, &ok.id, now()).unwrap_err()))
            .unwrap();
        assert!(matches!(
            err,
            FeedbackError::NotPending {
                status: ProposalStatus::Approved,
                ..
            }
        ));
    }

    #[test]
    fn slug_makes_valid_ids() {
        for (name, want) in [
            ("Agent Engineering!", "agent-engineering"),
            ("  Rust  ", "rust"),
            ("---", "topic"),
            ("Ünïcode Name", "n-code-name"),
        ] {
            let s = slug(name);
            assert_eq!(s, want);
            assert!(is_valid_id(&s), "{s}");
        }
        assert_eq!(slug(&"x".repeat(100)).len(), 64);
    }
}
