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
    let tz = parse_tz(&config.service.timezone).unwrap();
    AppState::new(db, config, tz, now, None)
}

async fn send(state: AppState, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, String) {
    let res = router(state).oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = res.into_body().collect().await.unwrap().to_bytes();
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

async fn get(db: Db, path: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    send(
        state(db),
        Request::builder().uri(path).body(Body::empty()).unwrap(),
    )
    .await
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

// ---------- Task 23: redirect, transcript, POST /run, /run/status ----------

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::http::header;
use dailybrief::core::embed::FakeEmbedder;
use dailybrief::harness::claude_code::ClaudeCodeAdapter;
use dailybrief::harness::service_runner::{ServiceRunner, ServiceRunnerOptions};
use dailybrief::harness::types::HarnessKind;

/// A state whose runner drives the fake `claude`; `hang` keeps the run alive for the 409 case.
fn state_with_runner(db: Db, tmp: &std::path::Path, hang: bool) -> AppState {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let data = tmp.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let data_s = data.to_string_lossy().into_owned();
    let config = load_config(
        &Env::from_lookup(|n| (n == "DAILYBRIEF_DATA_DIR").then(|| data_s.clone())).unwrap(),
    )
    .unwrap();
    let mut parent: HashMap<String, String> = HashMap::new();
    parent.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
    parent.insert("HOME".into(), "/tmp".into());
    parent.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), "fake".into());
    parent.insert(
        "DAILYBRIEF_FAKE_TRANSCRIPT".into(),
        root.join("tests/fixtures/transcripts/success.jsonl")
            .to_string_lossy()
            .into_owned(),
    );
    if hang {
        parent.insert("DAILYBRIEF_FAKE_HANG".into(), "1".into());
    }
    let mut a = ClaudeCodeAdapter::new(config.harness.claude_code.clone(), &parent).unwrap();
    a.binary = root.join("tests/fake-claude/claude");
    a.wall_clock = Duration::from_secs(5);
    let runner = ServiceRunner::with_harness(
        config.clone(),
        db.clone(),
        Vec::new(),
        Arc::new(FakeEmbedder),
        HarnessKind::ClaudeCode(a),
        ServiceRunnerOptions {
            verify: false,
            max_attempts: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let tz = parse_tz(&config.service.timezone).unwrap();
    AppState::new(db, config, tz, now, Some(Arc::new(runner)))
}

fn post_run(accept_json: bool, extra: &[(&str, &str)]) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri("/run")
        .header("host", "dailybrief.example");
    if accept_json {
        b = b.header(header::ACCEPT, "application/json");
    }
    for (k, v) in extra {
        b = b.header(*k, *v);
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn redirect_logs_read_and_302s_and_unknown_is_404() {
    let db = Db::open_in_memory().unwrap();
    let digest_id = published(&db);
    let (status, headers, _) = get(db.clone(), "/r/i03").await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(headers[header::LOCATION], "https://src0.example/i03");
    let (item, digest, at): (String, Option<String>, String) = db
        .with(|c| {
            Ok(
                c.query_row("SELECT item_id, digest_id, at FROM reads", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?,
            )
        })
        .unwrap();
    assert_eq!(item, "i03");
    assert_eq!(digest.as_deref(), Some(digest_id.as_str()));
    assert_eq!(at, "2026-09-16T23:45:00.000Z");
    let (status, _, _) = get(db, "/r/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn transcript_streams_file_and_unknown_is_404() {
    let db = Db::open_in_memory().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("transcript.jsonl");
    std::fs::write(&path, "{\"type\":\"system\"}\n{\"type\":\"result\"}\n").unwrap();
    db.with(|c| {
        repo::insert_run(
            c,
            &NewRun {
                id: "2026-09-17-t1".into(),
                kind: RunKind::Manual,
                harness: "claude-code".into(),
                attempt: 1,
                started_at: "2026-09-16T23:30:00.000Z".into(),
                transcript_path: Some(path.to_string_lossy().into_owned()),
            },
        )?;
        repo::insert_run(
            c,
            &NewRun {
                id: "2026-09-17-gone".into(),
                kind: RunKind::Manual,
                harness: "claude-code".into(),
                attempt: 1,
                started_at: "2026-09-16T23:31:00.000Z".into(),
                transcript_path: Some("/nonexistent/transcript.jsonl".into()),
            },
        )
    })
    .unwrap();
    let (status, headers, body) = get(db.clone(), "/runs/2026-09-17-t1/transcript").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/x-ndjson")
    );
    assert_eq!(body, "{\"type\":\"system\"}\n{\"type\":\"result\"}\n");
    let (status, _, _) = get(db.clone(), "/runs/2026-09-17-gone/transcript").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get(db, "/runs/unknown/transcript").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn post_run_503_without_runner_and_403_cross_origin() {
    let db = Db::open_in_memory().unwrap();
    let (status, _, body) = send(state(db.clone()), post_run(true, &[])).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("not enabled"));
    let tmp = tempfile::tempdir().unwrap();
    let st = state_with_runner(db.clone(), tmp.path(), false);
    let (status, _, _) = send(
        st.clone(),
        post_run(true, &[("sec-fetch-site", "cross-site")]),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, body) = send(st, post_run(false, &[("origin", "https://evil.example")])).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body.contains("cross-origin"));
}

#[tokio::test]
async fn post_run_passes_same_origin_and_no_origin_with_202_json_and_303_html() {
    let db = Db::open_in_memory().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let st = state_with_runner(db.clone(), tmp.path(), false);
    let (status, _, body) = send(
        st.clone(),
        post_run(true, &[("sec-fetch-site", "same-origin")]),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body, "{\"started\":true}");
    // Let the fake finish so the process-level guard is released.
    for _ in 0..100 {
        if !st.active.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (status, headers, _) = send(
        st.clone(),
        post_run(false, &[("origin", "https://dailybrief.example")]),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers[header::LOCATION], "/");
    for _ in 0..100 {
        if !st.active.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (status, _, _) = send(st, post_run(true, &[])).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "no Origin and no Sec-Fetch-Site: a non-browser client"
    );
}

#[tokio::test]
async fn post_run_429_after_cap() {
    let db = Db::open_in_memory().unwrap();
    for i in 0..3 {
        db.with(|c| {
            repo::insert_run(
                c,
                &NewRun {
                    id: format!("2026-09-17-m{i}"),
                    kind: RunKind::Manual,
                    harness: "claude-code".into(),
                    attempt: 1,
                    started_at: "2026-09-16T23:30:00.000Z".into(),
                    transcript_path: None,
                },
            )
        })
        .unwrap();
    }
    let tmp = tempfile::tempdir().unwrap();
    let st = state_with_runner(db, tmp.path(), false);
    let (status, _, body) = send(st, post_run(true, &[])).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(body.contains("3 per 24 h"));
}

#[tokio::test]
async fn post_run_409_when_active_and_run_status_reflects_it() {
    let db = Db::open_in_memory().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let st = state_with_runner(db, tmp.path(), true);
    let (status, _, body) = send(
        st.clone(),
        Request::builder()
            .uri("/run/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "{\"active\":false}");
    let (status, _, _) = send(st.clone(), post_run(true, &[])).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (_, _, body) = send(
        st.clone(),
        Request::builder()
            .uri("/run/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(body, "{\"active\":true}");
    let (status, _, body) = send(st.clone(), post_run(true, &[])).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body.contains("already in progress"));
    let (status, headers, _) = send(st, post_run(false, &[])).await;
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "browsers go home even when busy"
    );
    assert_eq!(headers[header::LOCATION], "/");
}
