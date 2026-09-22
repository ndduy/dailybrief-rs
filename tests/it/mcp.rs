//! Checkpoint D through the public API: the nine-tool trajectory, the select violation matrix,
//! the publish fatal flag, and the tools/list snapshot.

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use dailybrief::config::{Env, Feed, Topic, TopicOrigin, load_all};
use dailybrief::core::embed::{Embedder, FakeEmbedder};
use dailybrief::core::http::client;
use dailybrief::db::Db;
use dailybrief::db::repo::{self, NewItem, Role};
use dailybrief::mcp::server::DailyBriefServer;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, JsonObject};
use rmcp::service::RunningService;
use rmcp::{RoleClient, RoleServer};
use serde_json::{Value, json};

const RUN: &str = "2026-09-17-it-mcp";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 17, 6, 0, 0).unwrap()
}

struct Rig {
    client: RunningService<RoleClient, ()>,
    _server: RunningService<RoleServer, DailyBriefServer>,
    db: Db,
}

impl Rig {
    async fn start(db: Db) -> Self {
        Self::start_as(db, Role::Editor).await
    }

    async fn start_as(db: Db, role: Role) -> Self {
        let loaded = load_all(&Env::from_lookup(|_| None).unwrap()).unwrap();
        let server = DailyBriefServer::new(
            db.clone(),
            loaded.config.clone(),
            Vec::<Feed>::new(),
            RUN.into(),
            Arc::new(FakeEmbedder),
            client(&loaded.config.ingest).unwrap(),
            now,
        )
        .with_role(role);
        let (client_side, server_side) = tokio::io::duplex(1 << 16);
        let (sr, sw) = tokio::io::split(server_side);
        let (cr, cw) = tokio::io::split(client_side);
        let (server, client) = tokio::join!(server.serve((sr, sw)), ().serve((cr, cw)));
        Self {
            client: client.unwrap(),
            _server: server.unwrap(),
            db,
        }
    }

    async fn call(&self, name: &str, args: Value) -> CallToolResult {
        let mut params = CallToolRequestParams::new(name.to_string());
        params.arguments = match args {
            Value::Object(m) => Some(m),
            _ => None::<JsonObject>,
        };
        self.client.call_tool(params).await.unwrap()
    }

    async fn structured(&self, name: &str, args: Value) -> Value {
        let r = self.call(name, args).await;
        r.structured_content
            .unwrap_or_else(|| panic!("no structured content from {name}: {:?}", r.content))
    }

    async fn error_of(&self, name: &str, args: Value) -> String {
        let r = self.call(name, args).await;
        assert_eq!(r.is_error, Some(true), "{name} should fail: {r:?}");
        r.structured_content.unwrap()["error"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

/// 4 topics, 8 sources × 5 items with fake vectors.
fn seeded() -> Db {
    let db = Db::open_in_memory().unwrap();
    db.with(|c| {
        for t in 0..4 {
            repo::upsert_topic(
                c,
                &Topic {
                    id: format!("t{t}"),
                    name: format!("Topic {t}"),
                    description: String::new(),
                    weight: 1.0,
                    origin: TopicOrigin::Seed,
                },
            )?;
            let v = FakeEmbedder
                .embed(&[format!("Topic {t}")])
                .unwrap()
                .remove(0);
            repo::set_topic_vector(c, &format!("t{t}"), &v)?;
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
                let text = format!("Article {id} about topic {}.", k % 4);
                let v = FakeEmbedder
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
                        word_count: 5,
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
    db
}

fn select_args(id: &str, section: &str, topic: &str, reason: Option<&str>) -> Value {
    let mut v = json!({
        "section": section,
        "itemId": id,
        "summary": "A short summary.",
        "whyItMatters": "Because it does.",
        "topic": topic,
    });
    if let Some(r) = reason {
        v["reason"] = json!(r);
    }
    v
}

fn listing_of(tools: &[rmcp::model::Tool]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
            })
        })
        .collect()
}

#[tokio::test]
async fn curator_listing_names_exactly_the_five_tools() {
    let rig = Rig::start_as(seeded(), Role::Curator).await;
    let mut tools = rig.client.list_all_tools().await.unwrap();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(
        names,
        vec![
            "find_feeds",
            "get_feedback",
            "get_profile",
            "propose_change",
            "validate_feed"
        ]
    );
    insta::assert_json_snapshot!("tools_list_curator", listing_of(&tools));
    // The Editor's tools are not merely hidden: a call is an unknown tool.
    let mut params = CallToolRequestParams::new("publish_digest".to_string());
    params.arguments = None;
    assert!(rig.client.call_tool(params).await.is_err());
}

#[tokio::test]
async fn get_feedback_window_and_shapes() {
    let db = seeded();
    db.with(|c| {
        repo::insert_digest(
            c,
            &repo::DigestInsert {
                id: "d1".into(),
                date: "2026-09-15".into(),
                run_id: RUN.into(),
                published_at: "2026-09-15T00:00:00.000Z".into(),
                for_you_count: 1,
                beyond_radar_count: 1,
            },
            &[
                repo::DigestItemInsert {
                    item_id: "i00".into(),
                    section: repo::Section::ForYou,
                    position: 1,
                    summary: "s".into(),
                    why_it_matters: "w".into(),
                    reason: None,
                    topic: "Topic 0".into(),
                },
                repo::DigestItemInsert {
                    item_id: "i11".into(),
                    section: repo::Section::BeyondRadar,
                    position: 1,
                    summary: "s".into(),
                    why_it_matters: "w".into(),
                    reason: Some("adjacent_field".into()),
                    topic: "Topic 1".into(),
                },
            ],
        )?;
        repo::upsert_rating(
            c,
            "i00",
            Some("d1"),
            "down",
            "already_know",
            "2026-09-15T01:00:00.000Z",
        )?;
        repo::upsert_rating(
            c,
            "i11",
            Some("d1"),
            "up",
            "new_to_me",
            "2026-09-15T02:00:00.000Z",
        )?;
        repo::upsert_rating(
            c,
            "i22",
            None,
            "up",
            "deep_actionable",
            "2026-09-01T02:00:00.000Z",
        )?;
        repo::insert_read(c, "i11", Some("d1"), "2026-09-15T01:30:00.000Z")?;
        repo::insert_feed_issue(
            c,
            "src3",
            RUN,
            "junk",
            Some("listicles"),
            "2026-09-16T00:00:00.000Z",
        )?;
        Ok(())
    })
    .unwrap();
    let rig = Rig::start_as(db, Role::Curator).await;
    let fb = rig.structured("get_feedback", json!({})).await;
    assert_eq!(fb["window"]["from"], "2026-09-10T06:00:00.000Z");
    assert_eq!(fb["window"]["to"], "2026-09-17T06:00:00.000Z");
    let ratings = fb["ratings"].as_array().unwrap();
    assert_eq!(
        ratings.len(),
        2,
        "the 2026-09-01 rating is outside the window: {fb}"
    );
    assert_eq!(ratings[0]["itemId"], "i11");
    assert_eq!(ratings[0]["title"], "Title i11");
    assert_eq!(ratings[0]["source"], "SRC1");
    assert_eq!(ratings[0]["topic"], "Topic 1");
    assert_eq!(ratings[0]["sign"], "up");
    assert_eq!(ratings[0]["reason"], "new_to_me");
    assert_eq!(fb["reads"][0]["itemId"], "i11");
    assert_eq!(
        fb["exploreHitRate"],
        json!({"shown": 1, "read": 1, "ratedUp": 1})
    );
    assert_eq!(fb["feedIssues"][0]["feedId"], "src3");
    assert_eq!(fb["feedIssues"][0]["kind"], "junk");
    assert_eq!(fb["feedIssues"][0]["note"], "listicles");
}

#[tokio::test]
async fn get_profile_omits_topic_descriptions() {
    let db = seeded();
    db.with(|c| {
        repo::upsert_topic(
            c,
            &Topic {
                id: "secret".into(),
                name: "Plain Name".into(),
                description: "SECRET-DESCRIPTION-TEXT".into(),
                weight: 2.0,
                origin: TopicOrigin::Seed,
            },
        )?;
        repo::mark_source_failed(c, "src0", "HTTP 500")
    })
    .unwrap();
    let rig = Rig::start_as(db, Role::Curator).await;
    let p = rig.structured("get_profile", json!({})).await;
    assert!(!p.to_string().contains("SECRET-DESCRIPTION-TEXT"));
    let topics = p["topics"].as_array().unwrap();
    assert_eq!(topics.len(), 5);
    let secret = topics.iter().find(|t| t["id"] == "secret").unwrap();
    assert_eq!(secret["name"], "Plain Name");
    assert_eq!(secret["weight"], 2.0);
    assert_eq!(secret["origin"], "seed");
    assert!(secret.get("lastPositiveAt").is_some() && secret.get("saturation").is_some());
    let sources = p["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 8);
    let s0 = sources.iter().find(|s| s["id"] == "src0").unwrap();
    assert_eq!(s0["failures"], 1);
    assert_eq!(s0["url"], "https://src0.example/rss");
    assert!(s0.get("lastOkAt").is_some() && s0["enabled"] == true);
}

#[tokio::test]
async fn an_editor_server_has_no_propose_change() {
    let rig = Rig::start(seeded()).await;
    let mut params = CallToolRequestParams::new("propose_change".to_string());
    params.arguments = json!({"kind": "topic_weight", "payload": {}, "evidence": {"summary": "x"}})
        .as_object()
        .cloned();
    let err = rig.client.call_tool(params).await.unwrap_err();
    assert!(err.to_string().contains("tool not found"), "{err}");
}

/// The R0 listing, byte for byte (`editor_listing_is_unchanged`): the Curator's tools change
/// nothing the Editor sees.
#[tokio::test]
async fn tools_list_snapshot() {
    let rig = Rig::start(seeded()).await;
    let mut tools = rig.client.list_all_tools().await.unwrap();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(
        names,
        vec![
            "editor_notes",
            "fetch_sources",
            "get_briefing",
            "list_candidates",
            "publish_digest",
            "read_item",
            "report_feed_issue",
            "search_items",
            "select"
        ]
    );
    let listing: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.input_schema,
            })
        })
        .collect();
    insta::assert_json_snapshot!("tools_list", listing);
}

#[tokio::test]
async fn select_every_rejection_through_mcp_and_publish_third_rejection_is_fatal() {
    let rig = Rig::start(seeded()).await;
    // Unknown, then unread.
    assert_eq!(
        rig.error_of("select", select_args("nope", "for_you", "Topic 0", None))
            .await,
        "No item with id 'nope'. Use list_candidates or search_items."
    );
    assert_eq!(
        rig.error_of("select", select_args("i00", "for_you", "Topic 0", None))
            .await,
        "Read item 'i00' with read_item before selecting it."
    );
    rig.structured("read_item", json!({ "id": "i00" })).await;
    // Empty summary, long summary, unknown topic, missing reason, bad reason.
    let mut empty = select_args("i00", "for_you", "Topic 0", None);
    empty["summary"] = json!("  ");
    assert_eq!(rig.error_of("select", empty).await, "summary is required.");
    let mut long = select_args("i00", "for_you", "Topic 0", None);
    long["summary"] = json!("w ".repeat(81));
    assert_eq!(
        rig.error_of("select", long).await,
        "summary is 81 words; the cap is 80."
    );
    assert_eq!(
        rig.error_of("select", select_args("i00", "for_you", "Cooking", None))
            .await,
        "Unknown topic 'Cooking'. Known topics: Topic 0, Topic 1, Topic 2, Topic 3."
    );
    assert!(
        rig.error_of(
            "select",
            select_args("i00", "beyond_radar", "Topic 0", None)
        )
        .await
        .starts_with("beyond_radar needs a reason:")
    );
    assert!(
        rig.error_of(
            "select",
            select_args("i00", "beyond_radar", "Topic 0", Some("vibes"))
        )
        .await
        .starts_with("Unknown reason 'vibes'.")
    );
    // A schema-level rejection: bad section.
    let bad = rig
        .call("select", select_args("i00", "sideways", "Topic 0", None))
        .await;
    assert_eq!(bad.is_error, Some(true));
    // Stage it, then un-stage it.
    let staged = rig
        .structured("select", select_args("i00", "for_you", "Topic 0", None))
        .await;
    assert_eq!(
        staged,
        json!({ "ok": true, "replaced": false, "staged": { "forYou": 1, "beyondRadar": 0 } })
    );
    let gone = rig
        .structured("select", json!({ "section": "none", "itemId": "i00" }))
        .await;
    assert_eq!(gone["staged"]["forYou"], 0);

    // publish with nothing staged: rejection 1, 2, then 3 with fatal.
    for n in 1..=3 {
        let r = rig.call("publish_digest", json!({})).await;
        assert_eq!(r.is_error, Some(true));
        let body = r.structured_content.unwrap();
        assert_eq!(body["ok"], false);
        assert!(
            body["violations"][0]
                .as_str()
                .unwrap()
                .starts_with("for_you has 0 items")
        );
        assert_eq!(body.get("fatal").is_some(), n == 3, "rejection {n}: {body}");
    }
}

#[tokio::test]
async fn full_trajectory_through_mcp() {
    let rig = Rig::start(seeded()).await;
    let briefing = rig.structured("get_briefing", json!({})).await;
    assert_eq!(briefing["profile"]["topics"].as_array().unwrap().len(), 4);
    let exploit = rig
        .structured(
            "list_candidates",
            json!({ "strategy": "exploit", "limit": 80 }),
        )
        .await;
    let cands = exploit["candidates"].as_array().unwrap().clone();
    assert_eq!(cands.len(), 40);
    rig.structured(
        "list_candidates",
        json!({ "strategy": "popular_unmatched", "limit": 30 }),
    )
    .await;
    rig.structured(
        "list_candidates",
        json!({ "strategy": "cold_topic", "limit": 10 }),
    )
    .await;

    let mut staged = 0;
    let mut per_source = std::collections::HashMap::<String, usize>::new();
    for cand in &cands {
        if staged == 30 {
            break;
        }
        let id = cand["id"].as_str().unwrap();
        let read = rig.structured("read_item", json!({ "id": id })).await;
        assert!(!read["text"].as_str().unwrap().is_empty());
        let source = cand["source"].as_str().unwrap().to_string();
        let count = per_source.entry(source).or_default();
        if *count >= 4 {
            continue;
        }
        let (section, reason) = if staged < 24 {
            ("for_you", None)
        } else {
            ("beyond_radar", Some("emerging"))
        };
        let topic = format!("Topic {}", staged % 4);
        let out = rig
            .structured("select", select_args(id, section, &topic, reason))
            .await;
        assert_eq!(out["ok"], true);
        *count += 1;
        staged += 1;
    }
    assert_eq!(staged, 30);
    let published = rig.structured("publish_digest", json!({})).await;
    assert_eq!(published["ok"], true, "{published}");
    assert_eq!(published["forYou"], 24);
    assert_eq!(published["beyondRadar"], 6);
    assert_eq!(published["date"], "2026-09-17");
    let digest_id = published["digestId"].as_str().unwrap().to_string();
    let items = rig
        .db
        .with(|c| repo::list_digest_items(c, &digest_id))
        .unwrap();
    assert_eq!(items.len(), 30);
    let again = rig.call("publish_digest", json!({})).await;
    assert_eq!(again.is_error, Some(true));
    assert!(
        again.structured_content.unwrap()["violations"][0]
            .as_str()
            .unwrap()
            .starts_with("This run already published digest")
    );
}

/// Stages 24 + 6 items from the exploit list, the way the Editor does.
async fn stage_thirty(rig: &Rig) {
    let exploit = rig
        .structured(
            "list_candidates",
            json!({ "strategy": "exploit", "limit": 80 }),
        )
        .await;
    let cands = exploit["candidates"].as_array().unwrap().clone();
    let mut staged = 0;
    let mut per_source = std::collections::HashMap::<String, usize>::new();
    for cand in &cands {
        if staged == 30 {
            break;
        }
        let id = cand["id"].as_str().unwrap();
        rig.structured("read_item", json!({ "id": id })).await;
        let count = per_source
            .entry(cand["source"].as_str().unwrap().to_string())
            .or_default();
        if *count >= 4 {
            continue;
        }
        let (section, reason) = if staged < 24 {
            ("for_you", None)
        } else {
            ("beyond_radar", Some("emerging"))
        };
        let out = rig
            .structured(
                "select",
                select_args(id, section, &format!("Topic {}", staged % 4), reason),
            )
            .await;
        assert_eq!(out["ok"], true);
        *count += 1;
        staged += 1;
    }
    assert_eq!(staged, 30);
}

/// `spec/m2.md` §11 #7: after the third rejection the run is fatal; even a now-valid fourth
/// publish is refused and writes nothing.
#[tokio::test]
async fn publish_after_fatal_is_refused() {
    let rig = Rig::start(seeded()).await;
    for _ in 1..=3 {
        let r = rig.call("publish_digest", json!({})).await;
        assert_eq!(r.is_error, Some(true));
    }
    stage_thirty(&rig).await;
    let fourth = rig.call("publish_digest", json!({})).await;
    assert_eq!(fourth.is_error, Some(true), "{fourth:?}");
    let body = fourth.structured_content.unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["fatal"], true);
    assert!(
        body["violations"][0]
            .as_str()
            .unwrap()
            .contains("rejected 3 times"),
        "{body}"
    );
    let digests: i64 = rig
        .db
        .with(|c| Ok(c.query_row("SELECT count(*) FROM digests", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(digests, 0, "nothing was written");
}
