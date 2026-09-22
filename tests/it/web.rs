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
        body.len() < 24 * 1024,
        "HTML is {} bytes for 30 items (cap 24 KiB, spec/m3.md §11 #5)",
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
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(csp.starts_with("default-src 'none'"), "{csp}");
    assert!(csp.contains("script-src https://cdnjs.cloudflare.com"));
    assert!(!csp.contains("unsafe-eval"));
    assert!(csp.contains("frame-ancestors 'none'"));
    assert_eq!(headers["cache-control"], "no-store");
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
    state_with_runner_on(db, tmp, hang, "success.jsonl", None)
}

/// The same, replaying `transcript` and with an optional `--max-turns` override.
fn state_with_runner_on(
    db: Db,
    tmp: &std::path::Path,
    hang: bool,
    transcript: &str,
    max_turns: Option<u32>,
) -> AppState {
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
        root.join("tests/fixtures/transcripts")
            .join(transcript)
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
            max_turns,
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

#[tokio::test]
async fn runs_index_lists_newest_first_with_log_links() {
    let db = Db::open_in_memory().unwrap();
    run_row(&db, "2026-09-16-old1", RunStatus::Failed, Some("boom"));
    db.with(|c| {
        repo::insert_run(
            c,
            &NewRun {
                id: "2026-09-17-new1".into(),
                kind: RunKind::Manual,
                harness: "claude-code".into(),
                attempt: 1,
                started_at: "2026-09-17T01:00:00.000Z".into(),
                transcript_path: None,
            },
        )
    })
    .unwrap();
    let (status, _, body) = get(db, "/runs").await;
    assert_eq!(status, StatusCode::OK);
    let new = body
        .find("/runs/2026-09-17-new1/log")
        .expect("link to the newer run");
    let old = body
        .find("/runs/2026-09-16-old1/log")
        .expect("link to the older run");
    assert!(new < old, "newest first");
    assert!(body.contains("status-running") && body.contains("status-failed"));
    assert!(body.contains("boom"));
}

#[tokio::test]
async fn run_log_renders_events_in_order_reloads_while_running_and_unknown_is_404() {
    let db = Db::open_in_memory().unwrap();
    run_row(&db, "2026-09-17-done", RunStatus::Success, None);
    db.with(|c| {
        repo::insert_run(
            c,
            &NewRun {
                id: "2026-09-17-live".into(),
                kind: RunKind::Manual,
                harness: "claude-code".into(),
                attempt: 1,
                started_at: "2026-09-17T01:00:00.000Z".into(),
                transcript_path: None,
            },
        )?;
        repo::append_run_event(
            c,
            "2026-09-17-done",
            1,
            "system",
            r#"{"type":"system","subtype":"init","model":"claude-opus-5","tools":["WebSearch"],"mcp_servers":[{"name":"dailybrief","status":"connected"}]}"#,
        )?;
        repo::append_run_event(
            c,
            "2026-09-17-done",
            2,
            "assistant",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"mcp__dailybrief__get_briefing","input":{}}]}}"#,
        )?;
        repo::append_run_event(
            c,
            "2026-09-17-done",
            3,
            "result",
            r#"{"type":"result","subtype":"success","num_turns":3,"duration_ms":4000}"#,
        )?;
        Ok(())
    })
    .unwrap();
    let (status, _, body) = get(db.clone(), "/runs/2026-09-17-done/log").await;
    assert_eq!(status, StatusCode::OK);
    let init = body
        .find("init · model claude-opus-5 · 1 tools · mcp: dailybrief (connected)")
        .unwrap();
    let call = body.find("→ get_briefing {}").unwrap();
    let result = body.find("result · success · 3 turns · 4 s").unwrap();
    assert!(init < call && call < result, "events in seq order");
    assert!(body.contains("/runs/2026-09-17-done/transcript"));
    assert!(
        !body.contains("http-equiv=\"refresh\""),
        "finished runs do not reload"
    );

    let (status, _, body) = get(db.clone(), "/runs/2026-09-17-live/log").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("No events yet."));
    assert!(body.contains("http-equiv=\"refresh\" content=\"15\""));

    let (status, _, _) = get(db, "/runs/nope/log").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pages_are_full_width_and_link_to_the_runs_index_and_run_log() {
    let db = Db::open_in_memory().unwrap();
    published(&db);
    let (_, _, body) = get(db.clone(), "/d/2026-09-17").await;
    let css_body_rule = body
        .split("body{")
        .nth(1)
        .and_then(|s| s.split('}').next())
        .expect("a body rule in the inline CSS");
    assert!(
        !css_body_rule.contains("max-width"),
        "full width: {css_body_rule}"
    );
    assert!(body.contains("href=\"/runs\""));
    // run_row starts on 2026-09-17 local; a fresh database so no digest shadows the state.
    let db = Db::open_in_memory().unwrap();
    run_row(&db, "2026-09-17-fail", RunStatus::Failed, Some("x"));
    let (_, _, body) = get(db, "/d/2026-09-17").await;
    assert!(body.contains("/runs/2026-09-17-fail/log"));
}

/// A scheduled run holds only the database lock; POST /run must say 409 rather than 202 and
/// then quietly do nothing.
#[tokio::test]
async fn post_run_409_when_the_db_lock_is_held_by_a_scheduled_run() {
    let db = Db::open_in_memory().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    db.with(|c| {
        repo::try_acquire_lock(c, "scheduled@now", &now, "2000-01-01T00:00:00.000Z")?;
        Ok(())
    })
    .unwrap();
    let st = state_with_runner(db, tmp.path(), false);
    let (status, _, body) = send(st.clone(), post_run(true, &[])).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body.contains("scheduled@now"), "{body}");
    let (_, _, body) = send(
        st.clone(),
        Request::builder()
            .uri("/run/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["active"], true, "the scheduled run still holds the lock");
    assert_eq!(v["heldBy"], "scheduled@now");
    assert!(
        !st.active.load(std::sync::atomic::Ordering::SeqCst),
        "the in-process flag is released again"
    );
}

/// The only script is htmx from cdnjs with its SRI hash, and no inline handler that the CSP
/// (no `unsafe-eval`) would block; htmx gets an HX-Refresh answer instead.
#[tokio::test]
async fn page_script_has_sri_and_htmx_post_run_gets_hx_refresh() {
    let db = Db::open_in_memory().unwrap();
    published(&db);
    let (_, _, body) = get(db.clone(), "/d/2026-09-17").await;
    assert!(body.contains(
        "src=\"https://cdnjs.cloudflare.com/ajax/libs/htmx/2.0.4/htmx.min.js\" integrity=\"sha512-"
    ));
    assert!(body.contains("crossorigin=\"anonymous\""));
    assert!(!body.contains("hx-on"), "no inline handlers under the CSP");
    assert_eq!(body.matches("<script").count(), 1);

    let tmp = tempfile::tempdir().unwrap();
    let st = state_with_runner(db, tmp.path(), true);
    let (status, headers, _) = send(st.clone(), post_run(false, &[("hx-request", "true")])).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(headers["hx-refresh"], "true");
    let (status, headers, _) = send(st, post_run(false, &[("hx-request", "true")])).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(headers["hx-refresh"], "true");
}

// ---------- M2 Task 4: /runs/{id} ----------

/// Loads a run with events from a transcript file; `attempt`/`kind`/`status` as given.
fn run_with_events(db: &Db, id: &str, transcript: &std::path::Path, status: RunStatus) {
    run_row(db, id, status, None);
    let text = std::fs::read_to_string(transcript).unwrap();
    db.with(|c| {
        for (i, raw) in text.lines().filter(|l| !l.trim().is_empty()).enumerate() {
            let kind = dailybrief::harness::trajectory::event_type(raw);
            repo::append_run_event(c, id, i as i64 + 1, &kind, raw)?;
        }
        Ok(())
    })
    .unwrap();
}

fn real_fixture() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/transcripts/real-2026-09-21-c8354589.jsonl")
}

fn between(body: &str, start: &str, end: &str) -> String {
    let s = body.find(start).unwrap_or_else(|| panic!("no {start}"));
    let e = body[s..].find(end).map(|i| s + i + end.len()).unwrap();
    body[s..e].to_string()
}

#[tokio::test]
async fn run_page_renders_caps_retry_and_every_turn() {
    let db = Db::open_in_memory().unwrap();
    run_with_events(
        &db,
        "2026-09-21-c8354589",
        &real_fixture(),
        RunStatus::Success,
    );
    let (status, _, body) = get(db, "/runs/2026-09-21-c8354589").await;
    assert_eq!(status, StatusCode::OK);
    let bar = between(&body, "<div class=\"caps\"", "</div>");
    assert!(bar.contains("reads 32/45"), "{bar}");
    assert!(
        bar.contains("<span class=\"cap hit\">selects 31/30</span>"),
        "{bar}"
    );
    assert!(bar.contains("web 0/5"));
    assert!(bar.contains("turns 77/120"));
    assert!(bar.contains("8:20/15:00"), "{bar}");
    assert!(body.contains("attempt 1 of 2"), "single attempt, no retry");
    assert!(body.contains("href=\"/runs/2026-09-21-c8354589/log\""));
    assert!(body.contains("href=\"/runs/2026-09-21-c8354589/transcript\""));
    let table = between(&body, "<table class=\"turns\">", "</table>");
    assert_eq!(table.matches("<tr class=\"turn\">").count(), 77);
    insta::assert_snapshot!("run_page_turns", table);
}

#[tokio::test]
async fn run_page_for_a_running_run_reloads_and_unknown_run_is_404() {
    let db = Db::open_in_memory().unwrap();
    run_row(&db, "2026-09-17-live", RunStatus::Running, None);
    let (status, _, body) = get(db.clone(), "/runs/2026-09-17-live").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("http-equiv=\"refresh\" content=\"15\""));
    assert!(body.contains("turns 0/120"));
    let (status, _, _) = get(db, "/runs/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn runs_index_shows_turns_and_wall_and_links_the_run_page() {
    let db = Db::open_in_memory().unwrap();
    run_row(&db, "2026-09-17-done", RunStatus::Success, None);
    let (_, _, body) = get(db, "/runs").await;
    assert!(body.contains("href=\"/runs/2026-09-17-done\""), "{body}");
    assert!(body.contains("<td>12</td>"), "turns column from runs.turns");
    assert!(body.contains("<td>10:00</td>"), "wall from started/ended");
}

#[tokio::test]
async fn failed_state_shows_the_caps_bar_and_run_link() {
    let db = Db::open_in_memory().unwrap();
    let fake = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/transcripts/max-turns.jsonl");
    run_with_events(&db, "2026-09-17-fail", &fake, RunStatus::Failed);
    let (status, _, body) = get(db, "/d/2026-09-17").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("No digest"));
    assert!(body.contains("<div class=\"caps\""), "{body}");
    assert!(
        body.contains(".cap.hit{"),
        "the caps bar is styled on the day page too"
    );
    assert!(body.contains("href=\"/runs/2026-09-17-fail\""));
}

/// The retry chain is read by time window, so a pair of attempts keeps its chain no matter
/// how many newer runs exist.
#[tokio::test]
async fn retry_chain_for_a_run_outside_the_recent_rows() {
    let db = Db::open_in_memory().unwrap();
    let insert = |db: &Db, id: &str, attempt: i64, started: &str| {
        db.with(|c| {
            repo::insert_run(
                c,
                &NewRun {
                    id: id.into(),
                    kind: RunKind::Scheduled,
                    harness: "claude-code".into(),
                    attempt,
                    started_at: started.into(),
                    transcript_path: None,
                },
            )
        })
        .unwrap();
    };
    insert(&db, "2026-06-01-old1", 1, "2026-05-31T23:30:00.000Z");
    insert(&db, "2026-06-01-old2", 2, "2026-05-31T23:45:00.000Z");
    for i in 0..60 {
        insert(
            &db,
            &format!("2026-08-01-n{i:02}"),
            1,
            &format!("2026-08-01T{:02}:00:00.000Z", i % 24),
        );
    }
    let (status, _, body) = get(db, "/runs/2026-06-01-old2").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<ol class=\"chain\">"), "{body}");
    assert!(body.contains("href=\"/runs/2026-06-01-old1\""));
}

#[tokio::test]
async fn run_page_under_64_kib_for_120_turns() {
    let db = Db::open_in_memory().unwrap();
    run_row(&db, "2026-09-17-big", RunStatus::Success, None);
    db.with(|c| {
        let mut seq = 0;
        for i in 1..=120 {
            seq += 1;
            let a = format!(
                r#"{{"type":"assistant","timestamp":"2026-09-16T23:{:02}:{:02}.000Z","message":{{"id":"m{i}","content":[{{"type":"tool_use","id":"t{i}","name":"mcp__dailybrief__read_item","input":{{"id":"item{i:04}"}}}}],"usage":{{"input_tokens":3,"output_tokens":40,"cache_read_input_tokens":90000,"cache_creation_input_tokens":200}}}}}}"#,
                30 + i / 60,
                i % 60
            );
            repo::append_run_event(c, "2026-09-17-big", seq, "assistant", &a)?;
            seq += 1;
            let u = format!(
                r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t{i}","content":"{}"}}]}}}}"#,
                "x".repeat(5000)
            );
            repo::append_run_event(c, "2026-09-17-big", seq, "user", &u)?;
        }
        Ok(())
    })
    .unwrap();
    let (status, _, body) = get(db, "/runs/2026-09-17-big").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<span class=\"cap hit\">turns 120/120</span>"));
    assert!(body.len() < 64 * 1024, "{} bytes", body.len());
}

/// The M2 gate rehearsed against the fake: a run capped at five turns fails with
/// `error_max_turns`, the day page shows the failed state with `turns 5/5` marked hit and a
/// link to the run page, and the run page lists the five turns.
#[tokio::test]
async fn forced_failure_renders_turns_hit_on_home_and_run_page() {
    // The real transcript of the M2 gate run (2026-09-22-7494d2ef): five assistant messages,
    // `num_turns` 6 in the result line, `error_max_turns`.
    let db = Db::open_in_memory().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let st = state_with_runner_on(db.clone(), tmp.path(), false, "max-turns-5.jsonl", Some(5));
    let (status, _, _) = send(st.clone(), post_run(true, &[])).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if !st.active.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
    }
    assert!(
        !st.active.load(std::sync::atomic::Ordering::SeqCst),
        "run finished"
    );
    let run = db.with(|c| repo::latest_run(c)).unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Failed);
    let err = run.error.clone().unwrap_or_default();
    assert!(
        err.contains("max_turns") || err.contains("maximum number of turns"),
        "{err}"
    );
    let today = dailybrief::core::time::date_in_zone(Utc::now(), st.tz);
    let (status, _, body) = send(
        st.clone(),
        Request::builder()
            .uri(format!("/d/{today}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("No digest — run failed"), "{body}");
    assert!(
        body.contains("<span class=\"cap hit\">turns 5/5</span>"),
        "{body}"
    );
    assert!(body.contains(&format!("href=\"/runs/{}\"", run.id)));
    let (status, _, body) = send(
        st,
        Request::builder()
            .uri(format!("/runs/{}", run.id))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.matches("<tr class=\"turn\">").count(), 5);
    assert!(body.contains("<span class=\"cap hit\">turns 5/5</span>"));
}

/// `GET /run/status` sees a scheduled run, which holds only the database lock.
#[tokio::test]
async fn run_status_reflects_a_scheduled_run() {
    let db = Db::open_in_memory().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let st = state_with_runner(db.clone(), tmp.path(), false);
    let (_, _, body) = send(
        st.clone(),
        Request::builder()
            .uri("/run/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(body, "{\"active\":false}");
    let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    db.with(|c| {
        repo::try_acquire_lock(c, "scheduled@morning", &now, "2000-01-01T00:00:00.000Z")?;
        Ok(())
    })
    .unwrap();
    let (_, _, body) = send(
        st,
        Request::builder()
            .uri("/run/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["active"], true);
    assert_eq!(v["heldBy"], "scheduled@morning");
}

/// Two manual runs that each retried leave four rows; that is two runs against the cap of
/// three, so a third run is accepted and only a fourth is refused.
#[tokio::test]
async fn manual_run_cap_counts_runs_not_attempts() {
    let db = Db::open_in_memory().unwrap();
    let insert = |db: &Db, id: &str, attempt: i64| {
        db.with(|c| {
            repo::insert_run(
                c,
                &NewRun {
                    id: id.into(),
                    kind: RunKind::Manual,
                    harness: "claude-code".into(),
                    attempt,
                    started_at: "2026-09-16T23:30:00.000Z".into(),
                    transcript_path: None,
                },
            )
        })
        .unwrap();
    };
    insert(&db, "2026-09-17-a1", 1);
    insert(&db, "2026-09-17-a2", 2);
    insert(&db, "2026-09-17-b1", 1);
    insert(&db, "2026-09-17-b2", 2);
    let tmp = tempfile::tempdir().unwrap();
    let st = state_with_runner(db.clone(), tmp.path(), false);
    let (status, _, _) = send(st.clone(), post_run(true, &[])).await;
    assert_eq!(
        status,
        StatusCode::ACCEPTED,
        "two runs so far, cap is three"
    );
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if !st.active.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
    }
    insert(&db, "2026-09-17-c1", 1);
    let (status, _, _) = send(st, post_run(true, &[])).await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "the fourth run is over the cap"
    );
}

// ---------- M3 Task 3: ratings ----------

const DIGEST: &str = "2026-09-17-2026-09-17-abcd1234";

fn rate_req(form: &str, extra: &[(&str, &str)]) -> Request<Body> {
    let mut b = Request::builder()
        .method("POST")
        .uri("/rate")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("host", "dailybrief.example")
        .header("sec-fetch-site", "same-origin");
    for (k, v) in extra {
        b = b.header(*k, *v);
    }
    b.body(Body::from(form.to_string())).unwrap()
}

fn reasons_req(query: &str, htmx: bool) -> Request<Body> {
    let mut b = Request::builder().uri(format!("/rate/reasons?{query}"));
    if htmx {
        b = b.header("hx-request", "true");
    }
    b.body(Body::empty()).unwrap()
}

fn ratings(db: &Db) -> Vec<(String, String, String)> {
    db.with(|c| {
        Ok(repo::list_ratings_since(c, "2000-01-01T00:00:00.000Z")?
            .into_iter()
            .map(|r| (r.item_id, r.sign, r.reason))
            .collect())
    })
    .unwrap()
}

#[tokio::test]
async fn rate_stores_replaces_and_deletes() {
    let db = Db::open_in_memory().unwrap();
    published(&db);
    let before = db.with(|c| repo::row_counts(c)).unwrap();
    let form = "item=i00&sign=up&reason=new_to_me";
    let (status, _, body) =
        send(state(db.clone()), rate_req(form, &[("hx-request", "true")])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("👍 New to me") && body.contains("Undo"),
        "{body}"
    );
    assert!(!body.contains("<html"), "htmx gets the slot only");
    assert_eq!(
        ratings(&db),
        vec![("i00".to_string(), "up".to_string(), "new_to_me".to_string())]
    );
    let stored = db.with(|c| repo::get_rating(c, "i00")).unwrap().unwrap();
    assert_eq!(
        stored.digest_id.as_deref(),
        Some(DIGEST),
        "the newest digest the item was in"
    );
    assert_eq!(stored.at, "2026-09-16T23:45:00.000Z");
    let after = db.with(|c| repo::row_counts(c)).unwrap();
    let diff: Vec<_> = before.iter().zip(&after).filter(|(a, b)| a != b).collect();
    assert_eq!(diff.len(), 1, "only ratings changed: {diff:?}");
    assert_eq!(diff[0].1, &("ratings", 1));

    // A second rating replaces the first (one per item); a plain form post lands on the day.
    let form = "item=i00&sign=down&reason=too_shallow";
    let (status, headers, _) = send(state(db.clone()), rate_req(form, &[])).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers["location"], "/d/2026-09-17");
    assert_eq!(
        ratings(&db),
        vec![(
            "i00".to_string(),
            "down".to_string(),
            "too_shallow".to_string()
        )]
    );

    // The rated card shows the choice and the undo; the other 29 keep their two buttons.
    let (_, _, page) = get(db.clone(), "/d/2026-09-17").await;
    assert!(page.contains("👎 Too shallow"), "{page}");
    assert_eq!(page.matches("name=\"sign\" value=\"up\"").count(), 29);
    assert_eq!(page.matches("value=\"none\"").count(), 1, "one undo");

    // sign=none un-rates; the slot goes back to the two buttons.
    let form = "item=i00&sign=none";
    let (status, _, body) =
        send(state(db.clone()), rate_req(form, &[("hx-request", "true")])).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("value=\"up\"") && !body.contains("Undo"),
        "{body}"
    );
    assert!(ratings(&db).is_empty());
    assert_eq!(db.with(|c| repo::row_counts(c)).unwrap(), before);
}

#[tokio::test]
async fn rate_refuses_cross_origin_unknown_item_and_bad_reason() {
    let db = Db::open_in_memory().unwrap();
    published(&db);
    let ok = "item=i00&sign=up&reason=new_to_me";
    let cross = Request::builder()
        .method("POST")
        .uri("/rate")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("host", "dailybrief.example")
        .header("origin", "https://evil.example")
        .body(Body::from(ok))
        .unwrap();
    let (status, _, _) = send(state(db.clone()), cross).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let cases = [
        ("item=nope&sign=up&reason=new_to_me", StatusCode::NOT_FOUND),
        ("item=i00&sign=up&reason=off_topic", StatusCode::BAD_REQUEST),
        ("item=i00&sign=up", StatusCode::BAD_REQUEST),
        ("item=i00&sign=up&reason=brilliant", StatusCode::BAD_REQUEST),
        (
            "item=i00&sign=sideways&reason=new_to_me",
            StatusCode::BAD_REQUEST,
        ),
        ("sign=up&reason=new_to_me", StatusCode::BAD_REQUEST),
    ];
    for (form, want) in cases {
        let (status, _, body) = send(state(db.clone()), rate_req(form, &[])).await;
        assert_eq!(status, want, "{form}: {body}");
    }
    assert!(
        ratings(&db).is_empty(),
        "nothing stored by a refused request"
    );
}

#[tokio::test]
async fn digest_cards_carry_rating_controls_and_stay_under_24_kib() {
    let db = Db::open_in_memory().unwrap();
    published(&db);
    let (_, _, body) = get(db, "/d/2026-09-17").await;
    assert_eq!(body.matches("class=\"rate\"").count(), 30);
    assert_eq!(body.matches("name=\"sign\" value=\"up\"").count(), 30);
    assert_eq!(body.matches("name=\"sign\" value=\"down\"").count(), 30);
    assert_eq!(body.matches("hx-get=\"/rate/reasons\"").count(), 30);
    assert!(
        body.contains("action=\"/rate/reasons\""),
        "plain forms without htmx"
    );
    assert!(!body.contains("hx-on"), "no inline handlers under the CSP");
    assert!(body.len() < 24 * 1024, "HTML is {} bytes", body.len());
}

#[tokio::test]
async fn reasons_partial_lists_the_four_reasons_for_the_sign() {
    let db = Db::open_in_memory().unwrap();
    published(&db);
    let up = [
        "new_to_me",
        "deep_actionable",
        "relevant_to_current_work",
        "good_source",
    ];
    let down = ["already_know", "off_topic", "low_quality", "too_shallow"];
    let (status, _, body) = send(state(db.clone()), reasons_req("item=i00&sign=up", true)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("<html"), "htmx gets the partial only");
    assert_eq!(body.matches("name=\"reason\"").count(), 4);
    for r in up {
        assert!(
            body.contains(&format!("value=\"{r}\"")),
            "{r} missing: {body}"
        );
    }
    for r in down {
        assert!(
            !body.contains(&format!("value=\"{r}\"")),
            "{r} present: {body}"
        );
    }
    assert!(body.contains("hx-post=\"/rate\"") && body.contains("Cancel"));
    assert!(!body.contains("hx-on"));
    let (_, _, body) = send(state(db.clone()), reasons_req("item=i00&sign=down", true)).await;
    for r in down {
        assert!(
            body.contains(&format!("value=\"{r}\"")),
            "{r} missing: {body}"
        );
    }
    // sign=none is the cancel: the two buttons again.
    let (_, _, body) = send(state(db.clone()), reasons_req("item=i00&sign=none", true)).await;
    assert!(body.contains("name=\"sign\" value=\"up\"") && !body.contains("name=\"reason\""));
    // Without htmx the partial comes as a page (the plain-form fallback).
    let (status, _, body) = send(state(db.clone()), reasons_req("item=i00&sign=up", false)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html") && body.contains("value=\"new_to_me\""));
    let (status, _, _) = send(state(db.clone()), reasons_req("item=i00&sign=maybe", true)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = send(state(db.clone()), reasons_req("item=zz&sign=up", true)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(state(db), reasons_req("item=i00", true)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
