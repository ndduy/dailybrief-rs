//! The profile signal (`SPEC.md` §5): topic vectors weighted by `topics.weight` (damped when
//! saturated) plus the most recent reads decayed by recency rank. `exploit_score` is a number next
//! to a candidate, never a decision.

use std::sync::Arc;

use crate::config::Topic;
use crate::core::embed::Embedder;
use crate::core::vector::cosine;
use crate::db::{Connection, Db, DbError, repo};

/// Topics at or past this saturation count contribute half their weight.
pub const SATURATION_DAMP_AT: i64 = 3;
/// A read contributes this much at rank 1, decayed by `1 / (1 + log10(rank))`.
pub const READ_BASE_WEIGHT: f32 = 0.5;
/// How many recent reads enter the profile.
pub const READ_LIMIT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    Topic,
    Read,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileVector {
    pub kind: ProfileKind,
    pub id: String,
    pub weight: f32,
    pub vector: Vec<f32>,
}

/// What a topic is embedded from: `name + ". " + description`, or `name` alone.
pub fn topic_text(topic: &Topic) -> String {
    if topic.description.is_empty() {
        topic.name.clone()
    } else {
        format!("{}. {}", topic.name, topic.description)
    }
}

/// Mirrors `topics.toml` into `topics` and (re)embeds every entry locally: embed outside the DB
/// lock, then upsert + delete stale seed rows + store vectors in one `Db::call`.
pub async fn sync_topics(
    db: &Db,
    embedder: Arc<dyn Embedder>,
    topics: &[Topic],
) -> Result<(), DbError> {
    let texts: Vec<String> = topics.iter().map(topic_text).collect();
    let vectors = tokio::task::spawn_blocking(move || embedder.embed(&texts))
        .await
        .map_err(|_| DbError::Panicked)?
        .map_err(|e| DbError::Corrupt(format!("embedding topics failed: {e}")))?;
    let topics = topics.to_vec();
    db.call(move |conn| {
        for t in &topics {
            repo::upsert_topic(conn, t)?;
        }
        let keep: Vec<&str> = topics.iter().map(|t| t.id.as_str()).collect();
        repo::delete_seed_topics_not_in(conn, &keep)?;
        for (t, v) in topics.iter().zip(&vectors) {
            repo::set_topic_vector(conn, &t.id, v)?;
        }
        Ok(())
    })
    .await
}

/// `0.5 / (1 + log10(rank))`, rank 1 = the most recent read.
pub fn read_weight(rank: usize) -> f32 {
    READ_BASE_WEIGHT / (1.0 + (rank as f32).log10())
}

/// Every topic with a vector (weight × 0.5 when saturated) and the `READ_LIMIT` most recent reads.
pub fn profile_vectors(conn: &Connection) -> Result<Vec<ProfileVector>, DbError> {
    let mut out = Vec::new();
    for t in repo::list_topics(conn)? {
        let Some(vector) = t.vector else {
            continue;
        };
        let damp = if t.saturation >= SATURATION_DAMP_AT {
            0.5
        } else {
            1.0
        };
        out.push(ProfileVector {
            kind: ProfileKind::Topic,
            id: t.id,
            weight: t.weight as f32 * damp,
            vector,
        });
    }
    for (i, read) in repo::list_read_vectors(conn, READ_LIMIT)?
        .into_iter()
        .enumerate()
    {
        out.push(ProfileVector {
            kind: ProfileKind::Read,
            id: read.item_id,
            weight: read_weight(i + 1),
            vector: read.vector,
        });
    }
    Ok(out)
}

/// `Σ w·cos(item, v) / Σ|w|`; 0 for an empty profile.
pub fn exploit_score(vector: &[f32], profile: &[ProfileVector]) -> f32 {
    let (mut num, mut den) = (0.0f32, 0.0f32);
    for p in profile {
        num += p.weight * cosine(vector, &p.vector);
        den += p.weight.abs();
    }
    if den == 0.0 { 0.0 } else { num / den }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TopicOrigin;
    use crate::core::embed::FakeEmbedder;
    use crate::core::vector::DIMENSIONS;

    fn topic(id: &str, description: &str) -> Topic {
        Topic {
            id: id.into(),
            name: id.to_uppercase(),
            description: description.into(),
            weight: 1.0,
            origin: TopicOrigin::Seed,
        }
    }

    fn unit(i: usize) -> Vec<f32> {
        let mut v = vec![0.0; DIMENSIONS];
        v[i] = 1.0;
        v
    }

    #[test]
    fn topic_text_joins_name_and_description() {
        assert_eq!(topic_text(&topic("rust", "")), "RUST");
        assert_eq!(
            topic_text(&topic("rust", "systems language")),
            "RUST. systems language"
        );
    }

    #[tokio::test]
    async fn sync_topics_upserts_and_removes_stale_seed_rows() {
        let db = Db::open_in_memory().unwrap();
        let emb: Arc<dyn Embedder> = Arc::new(FakeEmbedder);
        sync_topics(&db, Arc::clone(&emb), &[topic("a", ""), topic("b", "")])
            .await
            .unwrap();
        db.with(|c| {
            let mut curated = topic("c", "");
            curated.origin = TopicOrigin::Curator;
            repo::upsert_topic(c, &curated)
        })
        .unwrap();
        sync_topics(&db, emb, &[topic("a", "changed")])
            .await
            .unwrap();
        let rows = db.with(|c| repo::list_topics(c)).unwrap();
        let ids: Vec<&str> = rows.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["a", "c"],
            "b (seed, gone from the file) removed; c (curator) kept"
        );
        assert_eq!(rows[0].description, "changed");
    }

    #[tokio::test]
    async fn sync_topics_embeds_name_and_description() {
        let db = Db::open_in_memory().unwrap();
        sync_topics(&db, Arc::new(FakeEmbedder), &[topic("a", "desc")])
            .await
            .unwrap();
        let rows = db.with(|c| repo::list_topics(c)).unwrap();
        let expected = FakeEmbedder
            .embed(&["A. desc".to_string()])
            .unwrap()
            .remove(0);
        assert_eq!(rows[0].vector.as_ref().unwrap(), &expected);
    }

    #[test]
    fn read_weight_decays_by_rank() {
        assert!((read_weight(1) - 0.5).abs() < 1e-6);
        assert!((read_weight(10) - 0.25).abs() < 1e-6);
        assert!(read_weight(2) < read_weight(1));
    }

    #[test]
    fn profile_weights_apply_saturation_damping_and_skip_missing_vectors() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            repo::upsert_topic(c, &topic("hot", ""))?;
            repo::set_topic_vector(c, "hot", &unit(0))?;
            let mut heavy = topic("sat", "");
            heavy.weight = 2.0;
            repo::upsert_topic(c, &heavy)?;
            repo::set_topic_vector(c, "sat", &unit(1))?;
            c.execute("UPDATE topics SET saturation = 3 WHERE id = 'sat'", [])?;
            repo::upsert_topic(c, &topic("novec", ""))?;
            Ok(())
        })
        .unwrap();
        let profile = db.with(|c| profile_vectors(c)).unwrap();
        let weights: Vec<(String, f32)> =
            profile.iter().map(|p| (p.id.clone(), p.weight)).collect();
        assert_eq!(
            weights,
            vec![("hot".to_string(), 1.0), ("sat".to_string(), 1.0)]
        );
    }

    #[test]
    fn profile_includes_recent_reads_ranked_by_recency() {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            crate::core::testutil::source(c, "s")?;
            for (id, i) in [("old", 2usize), ("new", 3)] {
                crate::core::testutil::item(c, id, "s", Some(unit(i)), "2026-09-16T00:00:00.000Z")?;
            }
            repo::insert_read(c, "old", None, "2026-09-16T01:00:00.000Z")?;
            repo::insert_read(c, "new", None, "2026-09-16T02:00:00.000Z")?;
            Ok(())
        })
        .unwrap();
        let profile = db.with(|c| profile_vectors(c)).unwrap();
        let reads: Vec<(&str, f32)> = profile
            .iter()
            .filter(|p| p.kind == ProfileKind::Read)
            .map(|p| (p.id.as_str(), p.weight))
            .collect();
        assert_eq!(reads.len(), 2);
        assert_eq!(reads[0].0, "new");
        assert!((reads[0].1 - 0.5).abs() < 1e-6);
        assert!(reads[1].1 < reads[0].1);
    }

    #[test]
    fn exploit_score_is_zero_for_empty_profile_and_weighted_otherwise() {
        assert_eq!(exploit_score(&unit(0), &[]), 0.0);
        let profile = vec![
            ProfileVector {
                kind: ProfileKind::Topic,
                id: "a".into(),
                weight: 3.0,
                vector: unit(0),
            },
            ProfileVector {
                kind: ProfileKind::Topic,
                id: "b".into(),
                weight: 1.0,
                vector: unit(1),
            },
        ];
        assert!((exploit_score(&unit(0), &profile) - 0.75).abs() < 1e-6);
        assert!((exploit_score(&unit(1), &profile) - 0.25).abs() < 1e-6);
    }
}
