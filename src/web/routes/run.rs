//! `POST /run` is the manual retry (`SPEC.md` §1 #11) and a billed action, so three guards sit in
//! front of it besides Access: a same-origin check (the Access cookie is SameSite=None, so a
//! cross-site form post would otherwise carry the session), a rolling 24 h cap, and one live run
//! per process (the database lock covers other processes).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;

use super::internal;
use crate::core::time::days_ago_iso;
use crate::db::repo::{self, RunKind};
use crate::web::app::AppState;

pub const MANUAL_RUNS_PER_DAY: i64 = 3;

/// Browser requests must come from this origin; non-browser clients (no `Origin`, no
/// `Sec-Fetch-Site`) pass.
pub fn is_same_origin(site: Option<&str>, origin: Option<&str>, host: Option<&str>) -> bool {
    if let Some(site) = site {
        return site == "same-origin" || site == "none";
    }
    let Some(origin) = origin else {
        return true;
    };
    match url::Url::parse(origin) {
        Ok(u) => {
            let origin_host = match u.port() {
                Some(p) => format!("{}:{p}", u.host_str().unwrap_or_default()),
                None => u.host_str().unwrap_or_default().to_string(),
            };
            host.is_some_and(|h| h == origin_host)
        }
        Err(_) => false,
    }
}

fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("application/json"))
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

pub async fn start(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let json = wants_json(&headers);
    // htmx sends HX-Request; an HX-Refresh answer makes it reload the page, so the running
    // state shows without any inline script (see the CSP in `web::app`).
    let htmx = header(&headers, "hx-request").is_some_and(|v| v == "true");
    let reply = |status: StatusCode, error: &str| -> Response {
        if json {
            (status, Json(json!({ "error": error }))).into_response()
        } else {
            (status, error.to_string()).into_response()
        }
    };
    // Browsers land back on the page whether the run started or one was already going.
    let reply_or_home = |status: StatusCode, body: serde_json::Value| -> Response {
        if json {
            (status, Json(body)).into_response()
        } else if htmx {
            (status, [("hx-refresh", "true")]).into_response()
        } else {
            (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response()
        }
    };
    let Some(runner) = state.runner.clone() else {
        return reply(
            StatusCode::SERVICE_UNAVAILABLE,
            "runs are not enabled on this instance",
        );
    };
    if !is_same_origin(
        header(&headers, "sec-fetch-site"),
        header(&headers, "origin"),
        header(&headers, "host"),
    ) {
        tracing::warn!("manual run refused: cross-origin request");
        return reply(
            StatusCode::FORBIDDEN,
            "cross-origin requests cannot start a run",
        );
    }
    let since = days_ago_iso((state.now)(), 1);
    let recent = match state
        .db
        .call(move |conn| repo::count_runs_since(conn, RunKind::Manual, &since))
        .await
    {
        Ok(n) => n,
        Err(e) => return internal(e),
    };
    if recent >= MANUAL_RUNS_PER_DAY {
        tracing::warn!(
            recent,
            cap = MANUAL_RUNS_PER_DAY,
            "manual run refused: daily cap reached"
        );
        return reply(
            StatusCode::TOO_MANY_REQUESTS,
            &format!("manual run cap reached ({MANUAL_RUNS_PER_DAY} per 24 h)"),
        );
    }
    if state
        .active
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return reply_or_home(
            StatusCode::CONFLICT,
            json!({ "error": "a run is already in progress" }),
        );
    }
    // A scheduled run (or the CLI) holds only the database lock: answer 409 now instead of
    // accepting a run that would be refused a moment later.
    match runner.lock_holder().await {
        Ok(None) => {}
        Ok(Some(holder)) => {
            state.active.store(false, Ordering::SeqCst);
            return reply_or_home(
                StatusCode::CONFLICT,
                json!({ "error": "a run is already in progress", "heldBy": holder }),
            );
        }
        Err(e) => {
            state.active.store(false, Ordering::SeqCst);
            return internal(e);
        }
    }
    let active = Arc::clone(&state.active);
    tokio::spawn(async move {
        match runner.run(RunKind::Manual).await {
            Ok(summary) => {
                tracing::info!(status = %summary.status, run_id = %summary.final_run_id, "manual run finished")
            }
            Err(e) => tracing::warn!(error = %e, "manual run did not run"),
        }
        active.store(false, Ordering::SeqCst);
    });
    reply_or_home(StatusCode::ACCEPTED, json!({ "started": true }))
}

/// `{ active, heldBy? }`: the in-process flag or the database lock (a scheduled run, or the
/// CLI, holds only the latter).
pub async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let flag = state.active.load(Ordering::SeqCst);
    let held_by = match state.runner.as_ref() {
        Some(runner) => runner.lock_holder().await.ok().flatten(),
        None => None,
    };
    match held_by {
        Some(holder) => Json(json!({ "active": true, "heldBy": holder })),
        None => Json(json!({ "active": flag })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_origin_rules() {
        assert!(is_same_origin(Some("same-origin"), None, None));
        assert!(is_same_origin(Some("none"), None, None));
        assert!(!is_same_origin(
            Some("cross-site"),
            Some("https://x"),
            Some("x")
        ));
        assert!(
            is_same_origin(None, None, Some("dailybrief.example")),
            "no Origin: non-browser client"
        );
        assert!(is_same_origin(
            None,
            Some("https://dailybrief.example"),
            Some("dailybrief.example")
        ));
        assert!(is_same_origin(
            None,
            Some("http://127.0.0.1:8788"),
            Some("127.0.0.1:8788")
        ));
        assert!(!is_same_origin(
            None,
            Some("https://evil.example"),
            Some("dailybrief.example")
        ));
        assert!(!is_same_origin(
            None,
            Some("not a url"),
            Some("dailybrief.example")
        ));
    }
}
