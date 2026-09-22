//! Checkpoint C: the whole in-process path — items → exploit → read ×30 → select ×30 → publish.

use chrono::{DateTime, TimeZone, Utc};
use dailybrief::config::{Env, Feed, Topic, TopicOrigin, load_config};
use dailybrief::core::candidates::{Strategy, list_candidates};
use dailybrief::core::digest::publish;
use dailybrief::core::embed::{Embedder, FakeEmbedder};
use dailybrief::core::staging::{SelectInput, read_item, stage_selection};
use dailybrief::core::time::parse_tz;
use dailybrief::db::Db;
use dailybrief::db::repo::{self, NewItem, Section};

const RUN: &str = "2026-09-17-scenario";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 17, 6, 0, 0).unwrap()
}

#[test]
fn full_run_in_process() {
    let config = load_config(&Env::from_lookup(|_| None).unwrap()).unwrap();
    let caps = config.caps;
    let tz = parse_tz(&config.service.timezone).unwrap();
    let db = Db::open_in_memory().unwrap();
    let embedder = FakeEmbedder;

    // 4 topics, 8 sources × 5 items = 40 items with vectors, one run row.
    db.with(|c| {
        for t in 0..4 {
            let topic = Topic {
                id: format!("t{t}"),
                name: format!("Topic {t}"),
                description: format!("about {t}"),
                weight: 1.0,
                origin: TopicOrigin::Seed,
            };
            repo::upsert_topic(c, &topic)?;
            let v = embedder
                .embed(&[format!("Topic {t}. about {t}")])
                .unwrap()
                .remove(0);
            repo::set_topic_vector(c, &topic.id, &v)?;
        }
        for s in 0..8 {
            let src = format!("src{s}");
            repo::upsert_source(
                c,
                &Feed {
                    id: src.clone(),
                    url: format!("https://{src}.example/rss"),
                    title: src.to_uppercase(),
                    weight: 1.0,
                    enabled: true,
                },
            )?;
            for k in 0..5 {
                let id = format!("i{s}{k}");
                let text = format!(
                    "Article {id} about topic {} with enough words to matter.",
                    k % 4
                );
                let v = embedder
                    .embed(std::slice::from_ref(&text))
                    .unwrap()
                    .remove(0);
                repo::insert_item(
                    c,
                    &NewItem {
                        id: id.clone(),
                        source_id: src.clone(),
                        url: format!("https://{src}.example/{id}"),
                        canonical_url: format!("https://{src}.example/{id}"),
                        title: format!("Title {id}"),
                        author: None,
                        published_at: Some("2026-09-16T08:00:00.000Z".into()),
                        fetched_at: "2026-09-16T09:00:00.000Z".into(),
                        text,
                        word_count: 11,
                        content_hash: format!("c{id}"),
                        title_hash: format!("t{id}"),
                        vector: Some(v),
                    },
                )?;
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
                started_at: "2026-09-17T06:00:00.000Z".into(),
                transcript_path: None,
            },
        )
    })
    .unwrap();

    // list_candidates(exploit, 80) → 40 candidates with scores.
    let candidates = db
        .with(|c| list_candidates(c, &caps, Strategy::Exploit, 80, now()))
        .unwrap();
    assert_eq!(candidates.len(), 40);
    assert!(candidates.iter().all(|c| !c.snippet.is_empty()));

    // read_item ×30, then select 24 + 6 respecting per-source (4) and per-topic (8) caps.
    let mut staged = 0;
    let mut per_source = std::collections::HashMap::<String, usize>::new();
    for cand in &candidates {
        if staged == 30 {
            break;
        }
        let read = db
            .with(|c| Ok(read_item(c, &caps, RUN, &cand.id)))
            .unwrap()
            .unwrap();
        assert!(!read.text.is_empty());
        let source_count = per_source.entry(cand.source.clone()).or_default();
        if *source_count >= 4 {
            continue;
        }
        let (section, reason) = if staged < 24 {
            (Section::ForYou, None)
        } else {
            (Section::BeyondRadar, Some("adjacent_field".to_string()))
        };
        let outcome = db
            .with(|c| {
                Ok(stage_selection(
                    c,
                    &caps,
                    RUN,
                    now(),
                    SelectInput::Stage {
                        section,
                        item_id: cand.id.clone(),
                        summary: format!("Summary for {}", cand.title),
                        why_it_matters: "It matters.".into(),
                        reason,
                        topic: format!("Topic {}", staged % 4),
                    },
                ))
            })
            .unwrap()
            .unwrap();
        assert!(!outcome.replaced);
        *source_count += 1;
        staged += 1;
    }
    assert_eq!(staged, 30);

    // publish_digest → 24 + 6 digest_items, the digest visible by run and by date.
    let published = db
        .with(|c| Ok(publish(c, &caps, tz, RUN, now())))
        .unwrap()
        .unwrap();
    assert_eq!((published.for_you, published.beyond_radar), (24, 6));
    assert_eq!(published.date, "2026-09-17");
    let items = db
        .with(|c| repo::list_digest_items(c, &published.digest_id))
        .unwrap();
    assert_eq!(items.len(), 30);
    assert!(
        items
            .iter()
            .filter(|i| i.section == Section::BeyondRadar)
            .all(|i| i.reason.is_some())
    );
    let by_date = db
        .with(|c| repo::latest_digest_for_date(c, "2026-09-17"))
        .unwrap()
        .unwrap();
    assert_eq!(by_date.id, published.digest_id);

    // Everything just shown is excluded from the next pool.
    let next = db
        .with(|c| list_candidates(c, &caps, Strategy::Exploit, 80, now()))
        .unwrap();
    assert_eq!(next.len(), 10);
}
