//! Shared fixtures for core tests: a source, an item with an optional vector, a shown digest row.

use crate::config::{Feed, Topic, TopicOrigin};
use crate::db::repo::{self, DigestInsert, DigestItemInsert, NewItem, Section};
use crate::db::{Connection, DbError};

pub fn source(conn: &Connection, id: &str) -> Result<(), DbError> {
    repo::upsert_source(
        conn,
        &Feed {
            id: id.to_string(),
            url: format!("https://{id}.example/rss"),
            title: id.to_uppercase(),
            weight: 1.0,
            enabled: true,
        },
    )
}

pub fn new_item(id: &str, source_id: &str, vector: Option<Vec<f32>>, fetched_at: &str) -> NewItem {
    NewItem {
        id: id.to_string(),
        source_id: source_id.to_string(),
        url: format!("https://{source_id}.example/{id}"),
        canonical_url: format!("https://{source_id}.example/{id}"),
        title: format!("Title {id}"),
        author: None,
        published_at: None,
        fetched_at: fetched_at.to_string(),
        text: format!("Body of {id}. Xin chào thế giới, this is the article text for {id}."),
        word_count: 12,
        content_hash: format!("c-{id}"),
        title_hash: format!("t-{id}"),
        vector,
    }
}

pub fn item(
    conn: &Connection,
    id: &str,
    source_id: &str,
    vector: Option<Vec<f32>>,
    fetched_at: &str,
) -> Result<(), DbError> {
    repo::insert_item(conn, &new_item(id, source_id, vector, fetched_at))
}

/// Publishes a one-item digest at `published_at` so `item_id` counts as shown.
pub fn shown(conn: &Connection, item_id: &str, published_at: &str) -> Result<(), DbError> {
    let run_id = format!("run-{item_id}");
    repo::insert_run(
        conn,
        &repo::NewRun {
            id: run_id.clone(),
            kind: repo::RunKind::Manual,
            harness: "test".into(),
            attempt: 1,
            started_at: published_at.to_string(),
            transcript_path: None,
        },
    )?;
    let digest_id = format!("digest-{item_id}");
    repo::insert_digest(
        conn,
        &DigestInsert {
            id: digest_id,
            date: published_at[..10].to_string(),
            run_id,
            published_at: published_at.to_string(),
            for_you_count: 1,
            beyond_radar_count: 0,
        },
        &[DigestItemInsert {
            item_id: item_id.to_string(),
            section: Section::ForYou,
            position: 1,
            summary: "s".into(),
            why_it_matters: "w".into(),
            reason: None,
            topic: "T".into(),
        }],
    )
}

/// A seed topic named `name` (no vector).
pub fn topic(conn: &Connection, id: &str, name: &str) -> Result<(), DbError> {
    repo::upsert_topic(
        conn,
        &Topic {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            weight: 1.0,
            origin: TopicOrigin::Seed,
        },
    )
}
