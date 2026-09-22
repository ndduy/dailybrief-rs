//! Every SQL statement in the crate. Functions are synchronous over `&Connection` and are composed
//! inside one `Db::call` closure by callers (ADR 0002). Row types mirror `SPEC.md` §5 columns.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::config::{Feed, Topic, TopicOrigin};
use crate::core::vector;

use super::DbError;

// ---------- sources ----------

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub id: String,
    pub url: String,
    pub title: String,
    pub weight: f64,
    pub enabled: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub failures: i64,
    pub last_ok_at: Option<String>,
    pub last_error: Option<String>,
}

fn map_source(row: &Row<'_>) -> rusqlite::Result<Source> {
    Ok(Source {
        id: row.get("id")?,
        url: row.get("url")?,
        title: row.get("title")?,
        weight: row.get("weight")?,
        enabled: row.get::<_, i64>("enabled")? != 0,
        etag: row.get("etag")?,
        last_modified: row.get("last_modified")?,
        failures: row.get("failures")?,
        last_ok_at: row.get("last_ok_at")?,
        last_error: row.get("last_error")?,
    })
}

const SOURCE_COLS: &str =
    "id, url, title, weight, enabled, etag, last_modified, failures, last_ok_at, last_error";

/// Mirrors a `feeds.toml` entry: inserts, or updates url/title/weight/enabled while keeping the
/// fetch state (etag, last_modified, failures, last_ok_at, last_error).
pub fn upsert_source(conn: &Connection, feed: &Feed) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO sources (id, url, title, weight, enabled) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET url = excluded.url, title = excluded.title,
             weight = excluded.weight, enabled = excluded.enabled",
        params![
            feed.id,
            feed.url,
            feed.title,
            feed.weight,
            i64::from(feed.enabled)
        ],
    )?;
    Ok(())
}

pub fn get_source(conn: &Connection, id: &str) -> Result<Option<Source>, DbError> {
    Ok(conn
        .query_row(
            &format!("SELECT {SOURCE_COLS} FROM sources WHERE id = ?1"),
            [id],
            map_source,
        )
        .optional()?)
}

pub fn list_sources(conn: &Connection) -> Result<Vec<Source>, DbError> {
    let mut stmt = conn.prepare(&format!("SELECT {SOURCE_COLS} FROM sources ORDER BY id"))?;
    let rows = stmt
        .query_map([], map_source)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// A successful fetch: stores the validators, resets failures, clears the last error.
pub fn mark_source_ok(
    conn: &Connection,
    id: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
    at: &str,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE sources SET etag = ?2, last_modified = ?3, failures = 0, last_ok_at = ?4, last_error = NULL
         WHERE id = ?1",
        params![id, etag, last_modified, at],
    )?;
    Ok(())
}

/// A failed fetch: increments failures and records the error.
pub fn mark_source_failed(conn: &Connection, id: &str, error: &str) -> Result<(), DbError> {
    conn.execute(
        "UPDATE sources SET failures = failures + 1, last_error = ?2 WHERE id = ?1",
        params![id, error],
    )?;
    Ok(())
}

// ---------- items ----------

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub source_id: String,
    pub url: String,
    pub canonical_url: String,
    pub title: String,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub fetched_at: String,
    pub text: String,
    pub word_count: i64,
    pub content_hash: String,
    pub title_hash: String,
    pub vector: Option<Vec<f32>>,
    pub social_score: Option<f64>,
}

impl Item {
    /// `published_at`, falling back to `fetched_at` — the ordering key for candidates.
    pub fn published_or_fetched(&self) -> &str {
        self.published_at.as_deref().unwrap_or(&self.fetched_at)
    }
}

/// Everything needed to insert an item; `id` is chosen by the caller (`core::dedupe::item_id`).
#[derive(Debug, Clone, PartialEq)]
pub struct NewItem {
    pub id: String,
    pub source_id: String,
    pub url: String,
    pub canonical_url: String,
    pub title: String,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub fetched_at: String,
    pub text: String,
    pub word_count: i64,
    pub content_hash: String,
    pub title_hash: String,
    pub vector: Option<Vec<f32>>,
}

const ITEM_COLS: &str = "id, source_id, url, canonical_url, title, author, published_at, fetched_at, \
                         text, word_count, content_hash, title_hash, vector, social_score";

fn decode_vector(row: &Row<'_>, col: &str) -> rusqlite::Result<Option<Vec<f32>>> {
    let blob: Option<Vec<u8>> = row.get(col)?;
    blob.map(|b| {
        vector::from_blob(&b).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Blob, Box::new(e))
        })
    })
    .transpose()
}

fn map_item(row: &Row<'_>) -> rusqlite::Result<Item> {
    Ok(Item {
        id: row.get("id")?,
        source_id: row.get("source_id")?,
        url: row.get("url")?,
        canonical_url: row.get("canonical_url")?,
        title: row.get("title")?,
        author: row.get("author")?,
        published_at: row.get("published_at")?,
        fetched_at: row.get("fetched_at")?,
        text: row.get("text")?,
        word_count: row.get("word_count")?,
        content_hash: row.get("content_hash")?,
        title_hash: row.get("title_hash")?,
        vector: decode_vector(row, "vector")?,
        social_score: row.get("social_score")?,
    })
}

/// Inserts one item. A duplicate `id` or `canonical_url` is a constraint error, by design.
pub fn insert_item(conn: &Connection, item: &NewItem) -> Result<(), DbError> {
    conn.execute(
        &format!(
            "INSERT INTO items ({ITEM_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, NULL)"
        ),
        params![
            item.id,
            item.source_id,
            item.url,
            item.canonical_url,
            item.title,
            item.author,
            item.published_at,
            item.fetched_at,
            item.text,
            item.word_count,
            item.content_hash,
            item.title_hash,
            item.vector.as_deref().map(vector::to_blob),
        ],
    )?;
    Ok(())
}

pub fn get_item(conn: &Connection, id: &str) -> Result<Option<Item>, DbError> {
    Ok(conn
        .query_row(
            &format!("SELECT {ITEM_COLS} FROM items WHERE id = ?1"),
            [id],
            map_item,
        )
        .optional()?)
}

pub fn has_canonical_url(conn: &Connection, canonical_url: &str) -> Result<bool, DbError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM items WHERE canonical_url = ?1",
            [canonical_url],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub fn has_title_hash(conn: &Connection, title_hash: &str) -> Result<bool, DbError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM items WHERE title_hash = ?1",
            [title_hash],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Like `has_title_hash`, within the dedupe window: a recurring title ("Weekly links") is a
/// duplicate for 14 days, not forever.
pub fn has_title_hash_since(
    conn: &Connection,
    title_hash: &str,
    since: &str,
) -> Result<bool, DbError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM items WHERE title_hash = ?1 AND fetched_at >= ?2",
            params![title_hash, since],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// `(id, vector)` for every item fetched at or after `since` that has a vector: the projection
/// the cosine dedupe compares against, loaded once per ingest batch.
pub fn list_item_vectors_since(
    conn: &Connection,
    since: &str,
) -> Result<Vec<(String, Vec<f32>)>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, vector FROM items WHERE fetched_at >= ?1 AND vector IS NOT NULL ORDER BY id",
    )?;
    let rows = stmt
        .query_map([since], |r| {
            let id: String = r.get(0)?;
            Ok((id, decode_vector(r, "vector")?.unwrap_or_default()))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Items fetched at or after `since` (stored-form timestamp), newest first, then id.
pub fn list_items_since(conn: &Connection, since: &str) -> Result<Vec<Item>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ITEM_COLS} FROM items WHERE fetched_at >= ?1 ORDER BY fetched_at DESC, id ASC"
    ))?;
    let rows = stmt
        .query_map([since], map_item)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn set_item_vector(conn: &Connection, id: &str, v: &[f32]) -> Result<(), DbError> {
    conn.execute(
        "UPDATE items SET vector = ?2 WHERE id = ?1",
        params![id, vector::to_blob(v)],
    )?;
    Ok(())
}

// ---------- topics ----------

#[derive(Debug, Clone, PartialEq)]
pub struct TopicRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub weight: f64,
    pub vector: Option<Vec<f32>>,
    pub origin: TopicOrigin,
    pub last_positive_at: Option<String>,
    pub saturation: i64,
}

const TOPIC_COLS: &str =
    "id, name, description, weight, vector, origin, last_positive_at, saturation";

fn map_topic(row: &Row<'_>) -> rusqlite::Result<TopicRow> {
    let origin: String = row.get("origin")?;
    let origin = match origin.as_str() {
        "seed" => TopicOrigin::Seed,
        "curator" => TopicOrigin::Curator,
        "explore-promoted" => TopicOrigin::ExplorePromoted,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!("unknown topic origin '{other}'").into(),
            ));
        }
    };
    Ok(TopicRow {
        id: row.get("id")?,
        name: row.get("name")?,
        description: row.get("description")?,
        weight: row.get("weight")?,
        vector: decode_vector(row, "vector")?,
        origin,
        last_positive_at: row.get("last_positive_at")?,
        saturation: row.get("saturation")?,
    })
}

/// Mirrors a `topics.toml` entry: inserts, or updates name/description/weight/origin while keeping
/// the learned state (vector, last_positive_at, saturation).
pub fn upsert_topic(conn: &Connection, topic: &Topic) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO topics (id, name, description, weight, origin) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, description = excluded.description,
             weight = excluded.weight, origin = excluded.origin",
        params![
            topic.id,
            topic.name,
            topic.description,
            topic.weight,
            topic.origin.as_str()
        ],
    )?;
    Ok(())
}

pub fn list_topics(conn: &Connection) -> Result<Vec<TopicRow>, DbError> {
    let mut stmt = conn.prepare(&format!("SELECT {TOPIC_COLS} FROM topics ORDER BY id"))?;
    let rows = stmt
        .query_map([], map_topic)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn set_topic_vector(conn: &Connection, id: &str, v: &[f32]) -> Result<(), DbError> {
    conn.execute(
        "UPDATE topics SET vector = ?2 WHERE id = ?1",
        params![id, vector::to_blob(v)],
    )?;
    Ok(())
}

/// Removes `seed` topics that are no longer in `topics.toml`; curator-made topics are kept.
pub fn delete_seed_topics_not_in(conn: &Connection, keep: &[&str]) -> Result<usize, DbError> {
    let mut stmt = conn.prepare("SELECT id FROM topics WHERE origin = 'seed'")?;
    let seeded = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut removed = 0;
    for id in seeded.iter().filter(|id| !keep.contains(&id.as_str())) {
        removed += conn.execute("DELETE FROM topics WHERE id = ?1", [id])?;
    }
    Ok(removed)
}

// ---------- runs, digests, reads (the parts other modules need now) ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Scheduled,
    Manual,
}

impl RunKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Manual => "manual",
        }
    }
}

/// Who the run is for (`runs.role`, migration 0002): the Editor builds a digest, the Curator
/// writes proposals. It selects the tool set the MCP server exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Editor,
    Curator,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Editor => "editor",
            Self::Curator => "curator",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "editor" => Some(Self::Editor),
            "curator" => Some(Self::Curator),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRun {
    pub id: String,
    pub kind: RunKind,
    pub role: Role,
    pub harness: String,
    pub attempt: i64,
    pub started_at: String,
    pub transcript_path: Option<String>,
}

/// Inserts a run in the `running` state.
pub fn insert_run(conn: &Connection, run: &NewRun) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO runs (id, kind, role, harness, status, attempt, started_at, transcript_path)
         VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6, ?7)",
        params![
            run.id,
            run.kind.as_str(),
            run.role.as_str(),
            run.harness,
            run.attempt,
            run.started_at,
            run.transcript_path
        ],
    )?;
    Ok(())
}

/// Digest sections (`SPEC.md` §5 CHECK constraint).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    ForYou,
    BeyondRadar,
}

impl Section {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ForYou => "for_you",
            Self::BeyondRadar => "beyond_radar",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "for_you" => Some(Self::ForYou),
            "beyond_radar" => Some(Self::BeyondRadar),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestInsert {
    pub id: String,
    pub date: String,
    pub run_id: String,
    pub published_at: String,
    pub for_you_count: i64,
    pub beyond_radar_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestItemInsert {
    pub item_id: String,
    pub section: Section,
    pub position: i64,
    pub summary: String,
    pub why_it_matters: String,
    pub reason: Option<String>,
    pub topic: String,
}

/// Writes the digest and its items in one transaction.
pub fn insert_digest(
    conn: &Connection,
    digest: &DigestInsert,
    items: &[DigestItemInsert],
) -> Result<(), DbError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO digests (id, date, run_id, published_at, for_you_count, beyond_radar_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            digest.id,
            digest.date,
            digest.run_id,
            digest.published_at,
            digest.for_you_count,
            digest.beyond_radar_count
        ],
    )?;
    for it in items {
        tx.execute(
            "INSERT INTO digest_items (digest_id, item_id, section, position, summary, why_it_matters, reason, topic)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                digest.id,
                it.item_id,
                it.section.as_str(),
                it.position,
                it.summary,
                it.why_it_matters,
                it.reason,
                it.topic
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Every item shown in a digest published at or after `since`.
pub fn shown_item_ids(
    conn: &Connection,
    since: &str,
) -> Result<std::collections::HashSet<String>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT di.item_id FROM digest_items di
         JOIN digests d ON d.id = di.digest_id
         WHERE d.published_at >= ?1",
    )?;
    let ids = stmt
        .query_map([since], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

/// A source click (`/r/{id}`).
pub fn insert_read(
    conn: &Connection,
    item_id: &str,
    digest_id: Option<&str>,
    at: &str,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO reads (item_id, digest_id, at) VALUES (?1, ?2, ?3)",
        params![item_id, digest_id, at],
    )?;
    Ok(())
}

/// Where a profile signal came from (ADR 0016: a 👍 is read-like scoring input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalKind {
    Read,
    Rating,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SignalVector {
    pub item_id: String,
    pub kind: SignalKind,
    pub vector: Vec<f32>,
}

/// Vectors of the most recent reads and positive ratings together (newest first, one window
/// and one ranking for both), for the profile.
pub fn list_profile_signal_vectors(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<SignalVector>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT x.item_id, x.kind, i.vector FROM (
             SELECT item_id, at, id, 'read' AS kind FROM reads
             UNION ALL
             SELECT item_id, at, id, 'rating' AS kind FROM ratings WHERE sign = 'up'
         ) x JOIN items i ON i.id = x.item_id
         WHERE i.vector IS NOT NULL ORDER BY x.at DESC, x.kind DESC, x.id DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit as i64], |row| {
            let kind: String = row.get(1)?;
            Ok(SignalVector {
                item_id: row.get(0)?,
                kind: if kind == "rating" {
                    SignalKind::Rating
                } else {
                    SignalKind::Read
                },
                vector: decode_vector(row, "vector")?.unwrap_or_default(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ---------- run_reads and selections (per-run staging, keyed by run id) ----------

/// Records that `item_id` was read in `run_id`; idempotent.
pub fn mark_run_read(conn: &Connection, run_id: &str, item_id: &str) -> Result<(), DbError> {
    conn.execute(
        "INSERT OR IGNORE INTO run_reads (run_id, item_id) VALUES (?1, ?2)",
        params![run_id, item_id],
    )?;
    Ok(())
}

pub fn list_run_reads(
    conn: &Connection,
    run_id: &str,
) -> Result<std::collections::HashSet<String>, DbError> {
    let mut stmt = conn.prepare("SELECT item_id FROM run_reads WHERE run_id = ?1")?;
    let ids = stmt
        .query_map([run_id], |r| r.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub item_id: String,
    pub section: Section,
    pub position: i64,
    pub summary: String,
    pub why_it_matters: String,
    pub reason: Option<String>,
    pub topic: String,
}

fn map_selection(row: &Row<'_>) -> rusqlite::Result<Selection> {
    let section: String = row.get("section")?;
    let section = Section::parse(&section).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown section '{section}'").into(),
        )
    })?;
    Ok(Selection {
        item_id: row.get("item_id")?,
        section,
        position: row.get("position")?,
        summary: row.get("summary")?,
        why_it_matters: row.get("why_it_matters")?,
        reason: row.get("reason")?,
        topic: row.get("topic")?,
    })
}

/// Stages or restages one item for a run.
pub fn upsert_selection(conn: &Connection, run_id: &str, s: &Selection) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO selections (run_id, item_id, section, position, summary, why_it_matters, reason, topic)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(run_id, item_id) DO UPDATE SET section = excluded.section,
             position = excluded.position, summary = excluded.summary,
             why_it_matters = excluded.why_it_matters, reason = excluded.reason, topic = excluded.topic",
        params![
            run_id,
            s.item_id,
            s.section.as_str(),
            s.position,
            s.summary,
            s.why_it_matters,
            s.reason,
            s.topic
        ],
    )?;
    Ok(())
}

pub fn delete_selection(conn: &Connection, run_id: &str, item_id: &str) -> Result<bool, DbError> {
    Ok(conn.execute(
        "DELETE FROM selections WHERE run_id = ?1 AND item_id = ?2",
        params![run_id, item_id],
    )? > 0)
}

/// Staged rows for a run, by section then position.
pub fn list_selections(conn: &Connection, run_id: &str) -> Result<Vec<Selection>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT item_id, section, position, summary, why_it_matters, reason, topic
         FROM selections WHERE run_id = ?1 ORDER BY section, position, item_id",
    )?;
    let rows = stmt
        .query_map([run_id], map_selection)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ---------- digests: reading back ----------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestRow {
    pub id: String,
    pub date: String,
    pub run_id: String,
    pub published_at: String,
    pub for_you_count: i64,
    pub beyond_radar_count: i64,
}

fn map_digest(row: &Row<'_>) -> rusqlite::Result<DigestRow> {
    Ok(DigestRow {
        id: row.get("id")?,
        date: row.get("date")?,
        run_id: row.get("run_id")?,
        published_at: row.get("published_at")?,
        for_you_count: row.get("for_you_count")?,
        beyond_radar_count: row.get("beyond_radar_count")?,
    })
}

const DIGEST_COLS: &str = "id, date, run_id, published_at, for_you_count, beyond_radar_count";

pub fn get_digest_by_run(conn: &Connection, run_id: &str) -> Result<Option<DigestRow>, DbError> {
    Ok(conn
        .query_row(
            &format!("SELECT {DIGEST_COLS} FROM digests WHERE run_id = ?1 ORDER BY published_at DESC LIMIT 1"),
            [run_id],
            map_digest,
        )
        .optional()?)
}

pub fn get_digest(conn: &Connection, id: &str) -> Result<Option<DigestRow>, DbError> {
    Ok(conn
        .query_row(
            &format!("SELECT {DIGEST_COLS} FROM digests WHERE id = ?1"),
            [id],
            map_digest,
        )
        .optional()?)
}

/// The latest digest published for a local date.
pub fn latest_digest_for_date(conn: &Connection, date: &str) -> Result<Option<DigestRow>, DbError> {
    Ok(conn
        .query_row(
            &format!("SELECT {DIGEST_COLS} FROM digests WHERE date = ?1 ORDER BY published_at DESC LIMIT 1"),
            [date],
            map_digest,
        )
        .optional()?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestItemRow {
    pub item_id: String,
    pub section: Section,
    pub position: i64,
    pub summary: String,
    pub why_it_matters: String,
    pub reason: Option<String>,
    pub topic: String,
}

/// A digest's items by section then position.
pub fn list_digest_items(
    conn: &Connection,
    digest_id: &str,
) -> Result<Vec<DigestItemRow>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT item_id, section, position, summary, why_it_matters, reason, topic
         FROM digest_items WHERE digest_id = ?1 ORDER BY section, position",
    )?;
    let rows = stmt
        .query_map([digest_id], |row| {
            let section: String = row.get("section")?;
            let section = Section::parse(&section).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    format!("unknown section '{section}'").into(),
                )
            })?;
            Ok(DigestItemRow {
                item_id: row.get("item_id")?,
                section,
                position: row.get("position")?,
                summary: row.get("summary")?,
                why_it_matters: row.get("why_it_matters")?,
                reason: row.get("reason")?,
                topic: row.get("topic")?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ---------- editor_notes (single row, harness-independent memory) ----------

pub fn get_editor_notes(conn: &Connection) -> Result<String, DbError> {
    Ok(conn
        .query_row("SELECT text FROM editor_notes WHERE id = 1", [], |r| {
            r.get(0)
        })
        .optional()?
        .unwrap_or_default())
}

pub fn set_editor_notes(conn: &Connection, text: &str, at: &str) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO editor_notes (id, text, updated_at) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET text = excluded.text, updated_at = excluded.updated_at",
        params![text, at],
    )?;
    Ok(())
}

// ---------- feed_issues ----------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedIssue {
    pub id: i64,
    pub source_id: String,
    pub run_id: Option<String>,
    pub kind: String,
    pub note: Option<String>,
    pub at: String,
}

pub fn insert_feed_issue(
    conn: &Connection,
    source_id: &str,
    run_id: &str,
    kind: &str,
    note: Option<&str>,
    at: &str,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO feed_issues (source_id, run_id, kind, note, at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![source_id, run_id, kind, note, at],
    )?;
    Ok(())
}

pub fn list_feed_issues(conn: &Connection, source_id: &str) -> Result<Vec<FeedIssue>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, source_id, run_id, kind, note, at FROM feed_issues WHERE source_id = ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map([source_id], |r| {
            Ok(FeedIssue {
                id: r.get("id")?,
                source_id: r.get("source_id")?,
                run_id: r.get("run_id")?,
                kind: r.get("kind")?,
                note: r.get("note")?,
                at: r.get("at")?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Stamps a reported issue on the source without touching failures or enablement.
pub fn set_source_last_error(conn: &Connection, id: &str, error: &str) -> Result<(), DbError> {
    conn.execute(
        "UPDATE sources SET last_error = ?2 WHERE id = ?1",
        params![id, error],
    )?;
    Ok(())
}

// ---------- runs: lifecycle, events, lock ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Success,
    Failed,
    Killed,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Killed => "killed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "running" => Some(Self::Running),
            "success" => Some(Self::Success),
            "failed" => Some(Self::Failed),
            "killed" => Some(Self::Killed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunRow {
    pub id: String,
    pub kind: String,
    pub role: Role,
    pub harness: String,
    pub status: RunStatus,
    pub attempt: i64,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub turns: Option<i64>,
    pub usage_json: Option<String>,
    pub cost_usd: Option<f64>,
    pub session_id: Option<String>,
    pub error: Option<String>,
    pub transcript_path: Option<String>,
}

const RUN_COLS: &str = "id, kind, role, harness, status, attempt, started_at, ended_at, turns, \
                        usage_json, cost_usd, session_id, error, transcript_path";

fn map_run(row: &Row<'_>) -> rusqlite::Result<RunRow> {
    let status: String = row.get("status")?;
    let status = RunStatus::parse(&status).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown run status '{status}'").into(),
        )
    })?;
    let role: String = row.get("role")?;
    let role = Role::parse(&role).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown run role '{role}'").into(),
        )
    })?;
    Ok(RunRow {
        id: row.get("id")?,
        kind: row.get("kind")?,
        role,
        harness: row.get("harness")?,
        status,
        attempt: row.get("attempt")?,
        started_at: row.get("started_at")?,
        ended_at: row.get("ended_at")?,
        turns: row.get("turns")?,
        usage_json: row.get("usage_json")?,
        cost_usd: row.get("cost_usd")?,
        session_id: row.get("session_id")?,
        error: row.get("error")?,
        transcript_path: row.get("transcript_path")?,
    })
}

pub fn get_run(conn: &Connection, id: &str) -> Result<Option<RunRow>, DbError> {
    Ok(conn
        .query_row(
            &format!("SELECT {RUN_COLS} FROM runs WHERE id = ?1"),
            [id],
            map_run,
        )
        .optional()?)
}

/// The most recently started run, if any.
pub fn latest_run(conn: &Connection) -> Result<Option<RunRow>, DbError> {
    Ok(conn
        .query_row(
            &format!("SELECT {RUN_COLS} FROM runs ORDER BY started_at DESC, id DESC LIMIT 1"),
            [],
            map_run,
        )
        .optional()?)
}

/// Runs started in `[from, to)`, newest first: the neighbours a retry chain is read from.
pub fn list_runs_between(conn: &Connection, from: &str, to: &str) -> Result<Vec<RunRow>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RUN_COLS} FROM runs WHERE started_at >= ?1 AND started_at < ?2 ORDER BY started_at DESC, id DESC"
    ))?;
    let rows = stmt
        .query_map([from, to], map_run)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The most recent `limit` runs, newest first.
pub fn list_runs(conn: &Connection, limit: i64) -> Result<Vec<RunRow>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RUN_COLS} FROM runs ORDER BY started_at DESC, id DESC LIMIT ?1"
    ))?;
    let rows = stmt
        .query_map([limit], map_run)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Finished runs (not `running`) that started before `before`, oldest first: the prune set.
pub fn list_runs_started_before(conn: &Connection, before: &str) -> Result<Vec<RunRow>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RUN_COLS} FROM runs WHERE started_at < ?1 AND status != 'running' ORDER BY started_at, id"
    ))?;
    let rows = stmt
        .query_map([before], map_run)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn count_run_events(conn: &Connection, run_id: &str) -> Result<i64, DbError> {
    Ok(conn.query_row(
        "SELECT count(*) FROM run_events WHERE run_id = ?1",
        [run_id],
        |r| r.get(0),
    )?)
}

/// Deletes a run's events (retention); the `runs` row is never deleted.
pub fn delete_run_events(conn: &Connection, run_id: &str) -> Result<usize, DbError> {
    Ok(conn.execute("DELETE FROM run_events WHERE run_id = ?1", [run_id])?)
}

/// Runs of `kind` started at or after `since`: first attempts only, so a retried run counts once.
pub fn count_runs_since(conn: &Connection, kind: RunKind, since: &str) -> Result<i64, DbError> {
    Ok(conn.query_row(
        "SELECT count(*) FROM runs WHERE kind = ?1 AND started_at >= ?2 AND attempt = 1",
        params![kind.as_str(), since],
        |r| r.get(0),
    )?)
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunFinish {
    pub status: RunStatus,
    pub ended_at: String,
    pub turns: Option<i64>,
    pub usage_json: Option<String>,
    pub cost_usd: Option<f64>,
    pub session_id: Option<String>,
    pub error: Option<String>,
}

pub fn finish_run(conn: &Connection, id: &str, f: &RunFinish) -> Result<(), DbError> {
    conn.execute(
        "UPDATE runs SET status = ?2, ended_at = ?3, turns = ?4, usage_json = ?5, cost_usd = ?6,
             session_id = ?7, error = ?8 WHERE id = ?1",
        params![
            id,
            f.status.as_str(),
            f.ended_at,
            f.turns,
            f.usage_json,
            f.cost_usd,
            f.session_id,
            f.error
        ],
    )?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEvent {
    pub seq: i64,
    pub kind: String,
    pub payload_json: String,
}

pub fn append_run_event(
    conn: &Connection,
    run_id: &str,
    seq: i64,
    kind: &str,
    payload_json: &str,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO run_events (run_id, seq, type, payload_json) VALUES (?1, ?2, ?3, ?4)",
        params![run_id, seq, kind, payload_json],
    )?;
    Ok(())
}

pub fn list_run_events(conn: &Connection, run_id: &str) -> Result<Vec<RunEvent>, DbError> {
    let mut stmt = conn
        .prepare("SELECT seq, type, payload_json FROM run_events WHERE run_id = ?1 ORDER BY seq")?;
    let rows = stmt
        .query_map([run_id], |r| {
            Ok(RunEvent {
                seq: r.get(0)?,
                kind: r.get(1)?,
                payload_json: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockResult {
    Acquired,
    Held { run_id: String, acquired_at: String },
}

/// Takes the single `run_lock` row unless a live holder exists; a holder older than
/// `stale_before` is taken over.
pub fn try_acquire_lock(
    conn: &Connection,
    holder: &str,
    now: &str,
    stale_before: &str,
) -> Result<LockResult, DbError> {
    // One statement, so two connections racing (the CLI against the service) cannot both win:
    // the upsert takes the row when it is absent or stale, and changes 0 rows otherwise.
    let changed = conn.execute(
        "INSERT INTO run_lock (id, run_id, acquired_at) VALUES (1, ?1, ?2) \
         ON CONFLICT(id) DO UPDATE SET run_id = excluded.run_id, acquired_at = excluded.acquired_at \
         WHERE run_lock.acquired_at < ?3",
        params![holder, now, stale_before],
    )?;
    if changed == 1 {
        return Ok(LockResult::Acquired);
    }
    match current_lock(conn)? {
        Some((run_id, acquired_at)) => Ok(LockResult::Held {
            run_id,
            acquired_at,
        }),
        // The holder released between our upsert and this read; the caller simply retries.
        None => Ok(LockResult::Held {
            run_id: String::new(),
            acquired_at: String::new(),
        }),
    }
}

/// The lock row as it is, if any: `(run_id, acquired_at)`.
pub fn current_lock(conn: &Connection) -> Result<Option<(String, String)>, DbError> {
    Ok(conn
        .query_row(
            "SELECT run_id, acquired_at FROM run_lock WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

pub fn release_lock(conn: &Connection) -> Result<(), DbError> {
    conn.execute("DELETE FROM run_lock WHERE id = 1", [])?;
    Ok(())
}

// ---------- reading surface: digest cards and runs of a day ----------

/// One digest item joined with the article and its source, as the page renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestCard {
    pub item_id: String,
    pub section: Section,
    pub position: i64,
    pub summary: String,
    pub why_it_matters: String,
    pub reason: Option<String>,
    pub topic: String,
    pub title: String,
    pub source: String,
    pub published_at: Option<String>,
}

pub fn list_digest_cards(conn: &Connection, digest_id: &str) -> Result<Vec<DigestCard>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT di.item_id, di.section, di.position, di.summary, di.why_it_matters, di.reason, di.topic,
                i.title, COALESCE(s.title, i.source_id) AS source, i.published_at
         FROM digest_items di
         JOIN items i ON i.id = di.item_id
         LEFT JOIN sources s ON s.id = i.source_id
         WHERE di.digest_id = ?1 ORDER BY di.section, di.position",
    )?;
    let rows = stmt
        .query_map([digest_id], |row| {
            let section: String = row.get("section")?;
            let section = Section::parse(&section).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    format!("unknown section '{section}'").into(),
                )
            })?;
            Ok(DigestCard {
                item_id: row.get("item_id")?,
                section,
                position: row.get("position")?,
                summary: row.get("summary")?,
                why_it_matters: row.get("why_it_matters")?,
                reason: row.get("reason")?,
                topic: row.get("topic")?,
                title: row.get("title")?,
                source: row.get("source")?,
                published_at: row.get("published_at")?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The most recently started run with `started_at` in `[from, to)`.
pub fn latest_run_between(
    conn: &Connection,
    from: &str,
    to: &str,
) -> Result<Option<RunRow>, DbError> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {RUN_COLS} FROM runs WHERE started_at >= ?1 AND started_at < ?2
                 ORDER BY started_at DESC, id DESC LIMIT 1"
            ),
            params![from, to],
            map_run,
        )
        .optional()?)
}

/// The newest run of one role that started in `[from, to)`; the day pages ask for the
/// Editor's so a Curator run never shows as the day's state.
pub fn latest_run_of_role_between(
    conn: &Connection,
    role: Role,
    from: &str,
    to: &str,
) -> Result<Option<RunRow>, DbError> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {RUN_COLS} FROM runs WHERE role = ?1 AND started_at >= ?2 AND started_at < ?3
                 ORDER BY started_at DESC, id DESC LIMIT 1"
            ),
            params![role.as_str(), from, to],
            map_run,
        )
        .optional()?)
}

/// The most recently published digest that showed `item_id`, if any.
pub fn latest_digest_for_item(conn: &Connection, item_id: &str) -> Result<Option<String>, DbError> {
    Ok(conn
        .query_row(
            "SELECT d.id FROM digest_items di JOIN digests d ON d.id = di.digest_id
             WHERE di.item_id = ?1 ORDER BY d.published_at DESC LIMIT 1",
            [item_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?)
}

/// Every item id, oldest fetched first (for `reembed`).
pub fn list_item_ids(conn: &Connection) -> Result<Vec<String>, DbError> {
    let mut stmt = conn.prepare("SELECT id FROM items ORDER BY fetched_at, id")?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    fn feed(id: &str) -> Feed {
        Feed {
            id: id.to_string(),
            url: format!("https://{id}.example/rss"),
            title: id.to_uppercase(),
            weight: 1.0,
            enabled: true,
        }
    }

    fn new_item(id: &str, source_id: &str, canonical: &str) -> NewItem {
        NewItem {
            id: id.to_string(),
            source_id: source_id.to_string(),
            url: format!("{canonical}?utm_source=x"),
            canonical_url: canonical.to_string(),
            title: format!("Title {id}"),
            author: None,
            published_at: Some("2026-09-16T00:00:00.000Z".to_string()),
            fetched_at: "2026-09-17T00:00:00.000Z".to_string(),
            text: "Xin chào thế giới".to_string(),
            word_count: 4,
            content_hash: format!("c{id}"),
            title_hash: format!("t{id}"),
            vector: None,
        }
    }

    fn topic(id: &str) -> Topic {
        Topic {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            weight: 1.0,
            origin: TopicOrigin::Seed,
        }
    }

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    #[test]
    fn upsert_source_keeps_fetch_state_and_updates_config_fields() {
        db().with(|c| {
            upsert_source(c, &feed("a"))?;
            mark_source_ok(
                c,
                "a",
                Some("W/\"e1\""),
                Some("Wed"),
                "2026-09-17T00:00:00.000Z",
            )?;
            let mut changed = feed("a");
            changed.title = "New".into();
            changed.weight = 2.5;
            changed.enabled = false;
            upsert_source(c, &changed)?;
            let s = get_source(c, "a")?.unwrap();
            assert_eq!(s.title, "New");
            assert_eq!(s.weight, 2.5);
            assert!(!s.enabled);
            assert_eq!(s.etag.as_deref(), Some("W/\"e1\""));
            assert_eq!(s.last_ok_at.as_deref(), Some("2026-09-17T00:00:00.000Z"));
            assert_eq!(list_sources(c)?.len(), 1);
            assert!(get_source(c, "nope")?.is_none());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn source_failures_count_and_reset() {
        db().with(|c| {
            upsert_source(c, &feed("a"))?;
            mark_source_failed(c, "a", "timeout")?;
            mark_source_failed(c, "a", "500")?;
            let s = get_source(c, "a")?.unwrap();
            assert_eq!(s.failures, 2);
            assert_eq!(s.last_error.as_deref(), Some("500"));
            mark_source_ok(c, "a", None, None, "2026-09-17T00:00:00.000Z")?;
            let s = get_source(c, "a")?.unwrap();
            assert_eq!(s.failures, 0);
            assert!(s.last_error.is_none());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn item_roundtrip_with_and_without_vector() {
        db().with(|c| {
            upsert_source(c, &feed("a"))?;
            let mut with = new_item("i1", "a", "https://a.example/1");
            with.vector = Some((0..vector::DIMENSIONS).map(|i| i as f32).collect());
            insert_item(c, &with)?;
            insert_item(c, &new_item("i2", "a", "https://a.example/2"))?;
            let got = get_item(c, "i1")?.unwrap();
            assert_eq!(got.vector.as_ref().map(Vec::len), Some(vector::DIMENSIONS));
            assert_eq!(got.text, "Xin chào thế giới");
            assert_eq!(got.published_or_fetched(), "2026-09-16T00:00:00.000Z");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn item_vector_null_when_absent() {
        db().with(|c| {
            upsert_source(c, &feed("a"))?;
            insert_item(c, &new_item("i1", "a", "https://a.example/1"))?;
            assert!(get_item(c, "i1")?.unwrap().vector.is_none());
            set_item_vector(c, "i1", &vec![0.5; vector::DIMENSIONS])?;
            assert_eq!(get_item(c, "i1")?.unwrap().vector.unwrap()[3], 0.5);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn upsert_item_conflicts_on_canonical_url() {
        db().with(|c| {
            upsert_source(c, &feed("a"))?;
            insert_item(c, &new_item("i1", "a", "https://a.example/1"))?;
            let err = insert_item(c, &new_item("i2", "a", "https://a.example/1")).unwrap_err();
            assert!(err.to_string().contains("UNIQUE"), "{err}");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn corrupt_vector_blob_is_an_error_not_a_panic() {
        db().with(|c| {
            upsert_source(c, &feed("a"))?;
            insert_item(c, &new_item("i1", "a", "https://a.example/1"))?;
            c.execute("UPDATE items SET vector = X'0102' WHERE id = 'i1'", [])?;
            let err = get_item(c, "i1").unwrap_err();
            assert!(err.to_string().contains("not a multiple of 4"), "{err}");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn lookups_by_canonical_url_and_title_hash() {
        db().with(|c| {
            upsert_source(c, &feed("a"))?;
            insert_item(c, &new_item("i1", "a", "https://a.example/1"))?;
            assert!(has_canonical_url(c, "https://a.example/1")?);
            assert!(!has_canonical_url(c, "https://a.example/2")?);
            assert!(has_title_hash(c, "ti1")?);
            assert!(!has_title_hash(c, "nope")?);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn list_items_since_filters_and_orders() {
        db().with(|c| {
            upsert_source(c, &feed("a"))?;
            let mut old = new_item("old", "a", "https://a.example/old");
            old.fetched_at = "2026-09-01T00:00:00.000Z".into();
            insert_item(c, &old)?;
            insert_item(c, &new_item("b2", "a", "https://a.example/b2"))?;
            insert_item(c, &new_item("a1", "a", "https://a.example/a1"))?;
            let ids: Vec<String> = list_items_since(c, "2026-09-10T00:00:00.000Z")?
                .into_iter()
                .map(|i| i.id)
                .collect();
            assert_eq!(ids, vec!["a1", "b2"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn upsert_topic_keeps_learned_state() {
        db().with(|c| {
            upsert_topic(c, &topic("rust"))?;
            set_topic_vector(c, "rust", &vec![1.0; vector::DIMENSIONS])?;
            c.execute("UPDATE topics SET saturation = 3, last_positive_at = '2026-09-01T00:00:00.000Z' WHERE id = 'rust'", [])?;
            let mut changed = topic("rust");
            changed.name = "Rust lang".into();
            changed.weight = 2.0;
            upsert_topic(c, &changed)?;
            let t = &list_topics(c)?[0];
            assert_eq!(t.name, "Rust lang");
            assert_eq!(t.weight, 2.0);
            assert_eq!(t.saturation, 3);
            assert!(t.vector.is_some());
            assert_eq!(t.last_positive_at.as_deref(), Some("2026-09-01T00:00:00.000Z"));
            assert_eq!(t.origin, TopicOrigin::Seed);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn delete_seed_topics_not_in_keeps_curator_topics() {
        db().with(|c| {
            upsert_topic(c, &topic("keep"))?;
            upsert_topic(c, &topic("drop"))?;
            let mut curated = topic("curated");
            curated.origin = TopicOrigin::Curator;
            upsert_topic(c, &curated)?;
            assert_eq!(delete_seed_topics_not_in(c, &["keep"])?, 1);
            let ids: Vec<String> = list_topics(c)?.into_iter().map(|t| t.id).collect();
            assert_eq!(ids, vec!["curated", "keep"]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn repo_is_the_only_sql_site() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && !path.to_string_lossy().contains("/src/db/")
                    && std::fs::read_to_string(&path).unwrap().contains("rusqlite")
                {
                    out.push(path.display().to_string());
                }
            }
        }
        walk(&root, &mut offenders);
        assert!(
            offenders.is_empty(),
            "rusqlite used outside src/db/: {offenders:?}"
        );
    }
}

// ---------- ratings and proposals (M3, ADR 0016 / 0018) ----------

/// Runs `f` inside one transaction: commit on `Ok`, roll back on `Err`. Callers outside
/// `db` use this instead of naming rusqlite (`repo_is_the_only_sql_site`).
pub fn in_transaction<T, E: From<DbError>>(
    conn: &Connection,
    f: impl FnOnce(&Connection) -> Result<T, E>,
) -> Result<T, E> {
    let tx = conn.unchecked_transaction().map_err(DbError::from)?;
    let out = f(&tx)?;
    tx.commit().map_err(DbError::from)?;
    Ok(out)
}

/// Every user table in the schema, with its row count. The "nothing else changed" assertions
/// diff two of these.
pub const TABLES: [&str; 11] = [
    "sources",
    "items",
    "topics",
    "reads",
    "digests",
    "digest_items",
    "runs",
    "run_events",
    "feed_issues",
    "ratings",
    "proposals",
];

pub fn row_counts(conn: &Connection) -> Result<Vec<(&'static str, i64)>, DbError> {
    let mut out = Vec::with_capacity(TABLES.len());
    for table in TABLES {
        let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
        out.push((table, n));
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq)]
pub struct RatingRow {
    pub id: i64,
    pub item_id: String,
    pub digest_id: Option<String>,
    pub sign: String,
    pub reason: String,
    pub at: String,
}

fn map_rating(row: &Row<'_>) -> rusqlite::Result<RatingRow> {
    Ok(RatingRow {
        id: row.get("id")?,
        item_id: row.get("item_id")?,
        digest_id: row.get("digest_id")?,
        sign: row.get("sign")?,
        reason: row.get("reason")?,
        at: row.get("at")?,
    })
}

const RATING_COLS: &str = "id, item_id, digest_id, sign, reason, at";

/// One rating per item: a second rating replaces the first.
pub fn upsert_rating(
    conn: &Connection,
    item_id: &str,
    digest_id: Option<&str>,
    sign: &str,
    reason: &str,
    at: &str,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO ratings (item_id, digest_id, sign, reason, at) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(item_id) DO UPDATE SET digest_id = excluded.digest_id, sign = excluded.sign, \
             reason = excluded.reason, at = excluded.at",
        params![item_id, digest_id, sign, reason, at],
    )?;
    Ok(())
}

pub fn delete_rating(conn: &Connection, item_id: &str) -> Result<usize, DbError> {
    Ok(conn.execute("DELETE FROM ratings WHERE item_id = ?1", [item_id])?)
}

pub fn get_rating(conn: &Connection, item_id: &str) -> Result<Option<RatingRow>, DbError> {
    Ok(conn
        .query_row(
            &format!("SELECT {RATING_COLS} FROM ratings WHERE item_id = ?1"),
            [item_id],
            map_rating,
        )
        .optional()?)
}

/// The ratings of the items in one digest (at most one per item).
pub fn list_ratings_for_digest(
    conn: &Connection,
    digest_id: &str,
) -> Result<Vec<RatingRow>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RATING_COLS} FROM ratings \
         WHERE item_id IN (SELECT item_id FROM digest_items WHERE digest_id = ?1)"
    ))?;
    let rows = stmt
        .query_map([digest_id], map_rating)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Ratings at or after `since`, newest first.
pub fn list_ratings_since(conn: &Connection, since: &str) -> Result<Vec<RatingRow>, DbError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RATING_COLS} FROM ratings WHERE at >= ?1 ORDER BY at DESC, id DESC"
    ))?;
    let rows = stmt
        .query_map([since], map_rating)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewProposal {
    pub id: String,
    pub run_id: Option<String>,
    pub kind: String,
    pub payload_json: String,
    pub evidence_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProposalRow {
    pub id: String,
    pub run_id: Option<String>,
    pub kind: String,
    pub payload_json: String,
    pub evidence_json: String,
    pub status: String,
    pub created_at: String,
    pub decided_at: Option<String>,
    pub applied_json: Option<String>,
}

const PROPOSAL_COLS: &str =
    "id, run_id, kind, payload_json, evidence_json, status, created_at, decided_at, applied_json";

fn map_proposal(row: &Row<'_>) -> rusqlite::Result<ProposalRow> {
    Ok(ProposalRow {
        id: row.get("id")?,
        run_id: row.get("run_id")?,
        kind: row.get("kind")?,
        payload_json: row.get("payload_json")?,
        evidence_json: row.get("evidence_json")?,
        status: row.get("status")?,
        created_at: row.get("created_at")?,
        decided_at: row.get("decided_at")?,
        applied_json: row.get("applied_json")?,
    })
}

pub fn insert_proposal(conn: &Connection, p: &NewProposal) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO proposals (id, run_id, kind, payload_json, evidence_json, status, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
        params![
            p.id,
            p.run_id,
            p.kind,
            p.payload_json,
            p.evidence_json,
            p.created_at
        ],
    )?;
    Ok(())
}

pub fn get_proposal(conn: &Connection, id: &str) -> Result<Option<ProposalRow>, DbError> {
    Ok(conn
        .query_row(
            &format!("SELECT {PROPOSAL_COLS} FROM proposals WHERE id = ?1"),
            [id],
            map_proposal,
        )
        .optional()?)
}

/// Proposals of one status, newest first (pending ones oldest first: the queue order).
pub fn list_proposals(
    conn: &Connection,
    status: crate::core::feedback::ProposalStatus,
    limit: i64,
) -> Result<Vec<ProposalRow>, DbError> {
    let order = if status == crate::core::feedback::ProposalStatus::Pending {
        "ASC"
    } else {
        "DESC"
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {PROPOSAL_COLS} FROM proposals WHERE status = ?1 ORDER BY created_at {order}, id {order} LIMIT ?2"
    ))?;
    let rows = stmt
        .query_map(params![status.as_str(), limit], map_proposal)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Moves a pending proposal to `status`; returns the rows changed (0 when it was not pending).
pub fn decide_proposal(
    conn: &Connection,
    id: &str,
    status: crate::core::feedback::ProposalStatus,
    decided_at: &str,
    applied_json: Option<&str>,
) -> Result<usize, DbError> {
    Ok(conn.execute(
        "UPDATE proposals SET status = ?2, decided_at = ?3, applied_json = ?4 \
         WHERE id = ?1 AND status = 'pending'",
        params![id, status.as_str(), decided_at, applied_json],
    )?)
}

// The five writers of topic and source state. Only `core::feedback::apply` may call them
// (`only_feedback_apply_writes_topics_and_sources`); `topics.toml` mirroring uses `upsert_topic`.

pub fn get_topic(conn: &Connection, id: &str) -> Result<Option<TopicRow>, DbError> {
    Ok(conn
        .query_row(
            &format!("SELECT {TOPIC_COLS} FROM topics WHERE id = ?1"),
            [id],
            map_topic,
        )
        .optional()?)
}

pub fn set_topic_weight(conn: &Connection, id: &str, weight: f64) -> Result<usize, DbError> {
    Ok(conn.execute(
        "UPDATE topics SET weight = ?2 WHERE id = ?1",
        params![id, weight],
    )?)
}

pub fn set_topic_origin(
    conn: &Connection,
    id: &str,
    origin: crate::config::TopicOrigin,
) -> Result<usize, DbError> {
    Ok(conn.execute(
        "UPDATE topics SET origin = ?2 WHERE id = ?1",
        params![id, origin.as_str()],
    )?)
}

pub fn insert_topic_from_proposal(
    conn: &Connection,
    id: &str,
    name: &str,
    description: &str,
    weight: f64,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO topics (id, name, description, weight, origin) VALUES (?1, ?2, ?3, ?4, 'curator')",
        params![id, name, description, weight],
    )?;
    Ok(())
}

pub fn set_source_enabled(conn: &Connection, id: &str, enabled: bool) -> Result<usize, DbError> {
    Ok(conn.execute(
        "UPDATE sources SET enabled = ?2 WHERE id = ?1",
        params![id, enabled],
    )?)
}

pub fn insert_source_from_proposal(
    conn: &Connection,
    id: &str,
    url: &str,
    title: &str,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO sources (id, url, title, weight, enabled) VALUES (?1, ?2, ?3, 1, 1)",
        params![id, url, title],
    )?;
    Ok(())
}

// ---------- Curator inputs (M3 Task 6): the week's evidence, read-only ----------

/// A rating with what the reader saw: the item's title, its source and the digest topic.
#[derive(Debug, Clone, PartialEq)]
pub struct RatingDetail {
    pub id: i64,
    pub item_id: String,
    pub title: String,
    pub source: String,
    pub topic: String,
    pub sign: String,
    pub reason: String,
    pub at: String,
}

/// Ratings at or after `since`, newest first, joined to the item and the digest row it was
/// rated from (topic empty when the rating carries no digest).
pub fn list_rating_details_since(
    conn: &Connection,
    since: &str,
) -> Result<Vec<RatingDetail>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.item_id, i.title, COALESCE(s.title, i.source_id), COALESCE(di.topic, ''),
                r.sign, r.reason, r.at
         FROM ratings r
         JOIN items i ON i.id = r.item_id
         LEFT JOIN sources s ON s.id = i.source_id
         LEFT JOIN digest_items di ON di.item_id = r.item_id AND di.digest_id = r.digest_id
         WHERE r.at >= ?1 ORDER BY r.at DESC, r.id DESC",
    )?;
    let rows = stmt
        .query_map([since], |row| {
            Ok(RatingDetail {
                id: row.get(0)?,
                item_id: row.get(1)?,
                title: row.get(2)?,
                source: row.get(3)?,
                topic: row.get(4)?,
                sign: row.get(5)?,
                reason: row.get(6)?,
                at: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadDetail {
    pub item_id: String,
    pub title: String,
    pub source: String,
    pub at: String,
}

/// Reads at or after `since`, newest first, with the item's title and source.
pub fn list_read_details_since(conn: &Connection, since: &str) -> Result<Vec<ReadDetail>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT r.item_id, i.title, COALESCE(s.title, i.source_id), r.at
         FROM reads r JOIN items i ON i.id = r.item_id LEFT JOIN sources s ON s.id = i.source_id
         WHERE r.at >= ?1 ORDER BY r.at DESC, r.id DESC",
    )?;
    let rows = stmt
        .query_map([since], |row| {
            Ok(ReadDetail {
                item_id: row.get(0)?,
                title: row.get(1)?,
                source: row.get(2)?,
                at: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// How the explore slots did: distinct beyond_radar items shown in digests published at or
/// after `since`, how many of them were read, how many rated up (any time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExploreStats {
    pub shown: i64,
    pub read: i64,
    pub rated_up: i64,
}

pub fn explore_stats_since(conn: &Connection, since: &str) -> Result<ExploreStats, DbError> {
    Ok(conn.query_row(
        "WITH shown AS (
             SELECT DISTINCT di.item_id FROM digest_items di
             JOIN digests d ON d.id = di.digest_id
             WHERE di.section = 'beyond_radar' AND d.published_at >= ?1
         )
         SELECT count(*),
                count(*) FILTER (WHERE EXISTS (SELECT 1 FROM reads r WHERE r.item_id = shown.item_id)),
                count(*) FILTER (WHERE EXISTS (SELECT 1 FROM ratings g WHERE g.item_id = shown.item_id AND g.sign = 'up'))
         FROM shown",
        [since],
        |row| {
            Ok(ExploreStats {
                shown: row.get(0)?,
                read: row.get(1)?,
                rated_up: row.get(2)?,
            })
        },
    )?)
}

/// Feed issues at or after `since`, newest first.
pub fn list_feed_issues_since(conn: &Connection, since: &str) -> Result<Vec<FeedIssue>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, source_id, run_id, kind, note, at FROM feed_issues
         WHERE at >= ?1 ORDER BY at DESC, id DESC",
    )?;
    let rows = stmt
        .query_map([since], |row| {
            Ok(FeedIssue {
                id: row.get(0)?,
                source_id: row.get(1)?,
                run_id: row.get(2)?,
                kind: row.get(3)?,
                note: row.get(4)?,
                at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
