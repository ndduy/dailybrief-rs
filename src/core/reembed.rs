//! `dailybrief reembed` (ADR 0005, `SPEC.md` §9 step 3): rewrite every item and topic vector with
//! the current embedder once, at first deployment. Vectors are never compared, only replaced.

use std::sync::Arc;

use serde::Serialize;

use super::embed::Embedder;
use super::ingest::embed_text;
use super::profile::sync_topics;
use crate::config::Topic;
use crate::db::{Db, DbError, repo};

/// Items per embedding batch: small enough to keep the DB lock short between batches.
const BATCH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReembedReport {
    pub items: usize,
    pub topics: usize,
}

/// Re-embeds every item (title + body) in batches, then re-syncs the topics.
pub async fn reembed_all(
    db: &Db,
    embedder: Arc<dyn Embedder>,
    topics: &[Topic],
) -> Result<ReembedReport, DbError> {
    let ids = db.call(|conn| repo::list_item_ids(conn)).await?;
    let mut done = 0;
    for chunk in ids.chunks(BATCH) {
        let batch_ids: Vec<String> = chunk.to_vec();
        let lookup = batch_ids.clone();
        let texts = db
            .call(move |conn| {
                let mut texts = Vec::with_capacity(lookup.len());
                for id in &lookup {
                    if let Some(item) = repo::get_item(conn, id)? {
                        texts.push(embed_text(&item.title, &item.text));
                    } else {
                        texts.push(String::new());
                    }
                }
                Ok(texts)
            })
            .await?;
        let embedder = Arc::clone(&embedder);
        let vectors = tokio::task::spawn_blocking(move || embedder.embed(&texts))
            .await
            .map_err(|_| DbError::Panicked)?
            .map_err(|e| DbError::Corrupt(format!("embedding failed: {e}")))?;
        let pairs: Vec<(String, Vec<f32>)> = batch_ids.into_iter().zip(vectors).collect();
        let n = pairs.len();
        db.call(move |conn| {
            for (id, v) in &pairs {
                repo::set_item_vector(conn, id, v)?;
            }
            Ok(())
        })
        .await?;
        done += n;
        tracing::info!(done, total = ids.len(), "reembed progress");
    }
    sync_topics(db, embedder, topics).await?;
    Ok(ReembedReport {
        items: done,
        topics: topics.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TopicOrigin;
    use crate::core::embed::FakeEmbedder;
    use crate::core::testutil::{item, source};
    use crate::core::vector::DIMENSIONS;

    #[tokio::test]
    async fn reembed_rewrites_every_vector() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            for i in 0..70 {
                let old = if i % 2 == 0 {
                    Some(vec![0.5; DIMENSIONS])
                } else {
                    None
                };
                item(c, &format!("i{i:02}"), "s", old, "2026-09-16T00:00:00.000Z")?;
            }
            Ok(())
        })
        .unwrap();
        let topics = vec![Topic {
            id: "rust".into(),
            name: "Rust".into(),
            description: String::new(),
            weight: 1.0,
            origin: TopicOrigin::Seed,
        }];
        let report = reembed_all(&db, Arc::new(FakeEmbedder), &topics)
            .await
            .unwrap();
        assert_eq!(
            report,
            ReembedReport {
                items: 70,
                topics: 1
            }
        );
        let items = db
            .with(|c| repo::list_items_since(c, "2026-01-01T00:00:00.000Z"))
            .unwrap();
        assert_eq!(items.len(), 70, "count unchanged");
        for it in &items {
            let v = it.vector.as_ref().expect("every item has a vector now");
            assert_ne!(v[0], 0.5, "the old vector was replaced");
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-4);
        }
        let t = db.with(|c| repo::list_topics(c)).unwrap();
        assert!(t[0].vector.is_some());
    }
}
