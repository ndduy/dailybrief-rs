//! The reading surface through axum's `oneshot`: every route, state and header.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, TimeZone, Utc};
use dailybrief::config::{Env, Feed, load_config};
use dailybrief::core::time::parse_tz;
use dailybrief::db::Db;
use dailybrief::db::repo::{
    self, DigestInsert, DigestItemInsert, NewItem, NewRun, RunFinish, RunKind, RunStatus, Section,
};
use dailybrief::web::app::{AppState, router};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn now() -> DateTime<Utc> {
    // 06:45 in Ho Chi Minh on 2026-09-17.
    Utc.with_ymd_and_hms(2026, 9, 16, 23, 45, 0).unwrap()
}

fn state(db: Db) -> AppState {
    let config = load_config(&Env::from_lookup(|_| None).unwrap()).unwrap();
    AppState {
        db,
        tz: parse_tz(&config.service.timezone).unwrap(),
        config,
        now,
    }
}

async fn get(db: Db, path: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let app = router(state(db));
    let res = app
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8(body.to_vec()).unwrap())
}

fn run_row(db: &Db, id: &str, status: RunStatus, error: Option<&str>) {
    db.with(|c| {
        repo::insert_run(
            c,
            &NewRun {
                id: id.into(),
                kind: RunKind::Scheduled,
                harness: "claude-code".into(),
                attempt: 1,
                started_at: "2026-09-16T23:30:00.000Z".into(),
                transcript_path: Some(format!("/data/runs/{id}/transcript.jsonl")),
            },
        )?;
        if status != RunStatus::Running {
            repo::finish_run(
                c,
                id,
                &RunFinish {
                    status,
                    ended_at: "2026-09-16T23:40:00.000Z".into(),
                    turns: Some(12),
                    usage_json: None,
                    cost_usd: None,
                    session_id: None,
                    error: error.map(str::to_string),
                },
            )?;
        }
        Ok(())
    })
    .unwrap();
}

/// A published digest with 24 + 6 items over 8 sources; summaries are realistic lengths.
fn published(db: &Db) -> String {
    let digest_id = "2026-09-17-2026-09-17-abcd1234".to_string();
    run_row(db, "2026-09-17-abcd1234", RunStatus::Success, None);
    db.with(|c| {
        let mut items = Vec::new();
        for s in 0..8 {
            let src = format!("src{s}");
            repo::upsert_source(
                c,
                &Feed {
                    id: src.clone(),
                    url: format!("https://{src}.example/rss"),
                    title: format!("Source {s}"),
                    weight: 1.0,
                    enabled: true,
                },
            )?;
            for k in 0..4 {
                let n = s * 4 + k;
                if n >= 30 {
                    break;
                }
                let id = format!("i{n:02}");
                repo::insert_item(
                    c,
                    &NewItem {
                        id: id.clone(),
                        source_id: src.clone(),
                        url: format!("https://{src}.example/{id}"),
                        canonical_url: format!("https://{src}.example/{id}"),
                        title: format!("Article {n}: a title of ordinary length about something"),
                        author: None,
                        published_at: Some("2026-09-16T08:00:00.000Z".into()),
                        fetched_at: "2026-09-16T09:00:00.000Z".into(),
                        text: "body".into(),
                        word_count: 1,
                        content_hash: format!("c{id}"),
                        title_hash: format!("t{id}"),
                        vector: None,
                    },
                )?;
                let beyond = n >= 24;
                items.push(DigestItemInsert {
                    item_id: id,
                    section: if beyond { Section::BeyondRadar } else { Section::ForYou },
                    position: if beyond { n as i64 - 23 } else { n as i64 + 1 },
                    summary: "A summary of roughly forty words that explains what the article says, why the author wrote it, what evidence they bring, and what a reader in a hurry should take away from it this morning.".into(),
                    why_it_matters: "It changes how you would approach the next design review.".into(),
                    reason: beyond.then(|| "adjacent_field".to_string()),
                    topic: format!("Topic {}", n % 4),
                });
            }
        }
        repo::insert_digest(
            c,
            &DigestInsert {
                id: digest_id.clone(),
                date: "2026-09-17".into(),
                run_id: "2026-09-17-abcd1234".into(),
                published_at: "2026-09-16T23:40:00.000Z".into(),
                for_you_count: 24,
                beyond_radar_count: 6,
            },
            &items,
        )
    })
    .unwrap();
    digest_id
}

#[tokio::test]
async fn root_renders_today_and_day_renders_published_digest_with_24_then_6() {
    let db = Db::open_in_memory().unwrap();
    published(&db);
    let (status, _, body) = get(db.clone(), "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("2026-09-17"));
    let (status, _, body) = get(db, "/d/2026-09-17").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.matches("<article>").count(), 30);
    let for_you = body.find("For you").unwrap();
    let beyond = body.find("Beyond your radar").unwrap();
    assert!(for_you < beyond);
    assert_eq!(body[for_you..beyond].matches("<article>").count(), 24);
    assert_eq!(body[beyond..].matches("<article>").count(), 6);
    assert_eq!(
        body.matches("class=\"badge\"").count(),
        6,
        "beyond_radar cards show the reason badge"
    );
    assert!(body.contains("adjacent_field"));
    assert!(body.contains("href=\"/r/i00\""));
    assert!(body.contains("Source 0"));
    assert!(body.contains("Topic 1"));
    assert!(body.contains("hx-post=\"/run\""));
    assert!(
        body.len() < 15 * 1024,
        "HTML is {} bytes for 30 items",
        body.len()
    );
}

#[tokio::test]
async fn day_renders_failed_running_and_no_run_states() {
    let db = Db::open_in_memory().unwrap();
    run_row(
        &db,
        "2026-09-17-failed01",
        RunStatus::Failed,
        Some("Reached maximum number of turns (120)"),
    );
    let (status, _, body) = get(db.clone(), "/d/2026-09-17").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("No digest — run failed."));
    assert!(body.contains("Reached maximum number of turns (120)"));
    assert!(body.contains("href=\"/runs/2026-09-17-failed01/transcript\""));
    assert!(
        body.contains("hx-post=\"/run\""),
        "failed state offers Refresh"
    );

    let db = Db::open_in_memory().unwrap();
    run_row(&db, "2026-09-17-running1", RunStatus::Running, None);
    let (_, _, body) = get(db, "/d/2026-09-17").await;
    assert!(body.contains("Run in progress."));
    assert!(!body.contains("hx-post"), "no Refresh while a run is live");

    let db = Db::open_in_memory().unwrap();
    let (status, _, body) = get(db.clone(), "/d/2026-09-17").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("No run yet."));
    // A run on another local day does not count for this one.
    run_row(&db, "2026-09-17-other001", RunStatus::Failed, Some("x"));
    let (_, _, body) = get(db, "/d/2026-09-18").await;
    assert!(body.contains("No run yet."));
}

#[tokio::test]
async fn unknown_route_bad_date_and_security_headers() {
    let db = Db::open_in_memory().unwrap();
    let (status, headers, _) = get(db.clone(), "/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(headers["referrer-policy"], "no-referrer");
    assert_eq!(headers["x-frame-options"], "DENY");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    let (status, headers, body) = get(db.clone(), "/d/yesterday").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("is not a date"));
    assert_eq!(headers["x-frame-options"], "DENY");
    let (status, _, _) = get(db, "/d/2026-02-30").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
