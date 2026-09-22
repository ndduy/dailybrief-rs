//! `/runs` (index) and `/runs/{id}` (one run as turns, caps and retry chain, ADR 0012).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use super::internal;
use crate::config::Config;
use crate::core::time::{parse_iso, to_iso};
use crate::db::repo::{self, RunEvent};
use crate::harness::trajectory::{
    AttemptOutcome, CapsUsed, Event, caps_used, fold_turns, parse_line, retry_chain,
};
use crate::web::app::AppState;
use crate::web::views::run::{render_index, render_run};

/// Rows on the runs index.
pub const INDEX_LIMIT: i64 = 30;

/// What a run consumed, from its stored events and the configured caps.
pub fn caps_of(config: &Config, events: &[RunEvent]) -> CapsUsed {
    let turns = fold_turns(events);
    let result = events
        .iter()
        .rev()
        .find_map(|e| match parse_line(&e.payload_json) {
            Event::Result(r) => Some(r),
            _ => None,
        });
    caps_used(
        &turns,
        result.as_ref(),
        &config.caps,
        &config.harness.claude_code,
    )
}

/// The lock window (`SPEC.md` §3): attempts of one summary start within it.
fn chain_window_secs(config: &Config) -> i64 {
    i64::from(2 * config.harness.claude_code.wall_clock_minutes + 5) * 60
}

/// `[started - window, started + window)` as stored-form timestamps; a run whose timestamp
/// does not parse gets an empty window and no chain.
fn chain_bounds(started_at: &str, window_secs: i64) -> (String, String) {
    match parse_iso(started_at) {
        Some(t) => (
            to_iso(t - chrono::Duration::seconds(window_secs)),
            to_iso(t + chrono::Duration::seconds(window_secs)),
        ),
        None => (started_at.to_string(), started_at.to_string()),
    }
}

pub async fn index(State(state): State<AppState>) -> Response {
    match state
        .db
        .call(|conn| repo::list_runs(conn, INDEX_LIMIT))
        .await
    {
        Ok(runs) => Html(render_index(&runs).into_string()).into_response(),
        Err(e) => internal(e),
    }
}

pub async fn show(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let lookup = id.clone();
    let window = chain_window_secs(&state.config);
    let loaded = state
        .db
        .call(move |conn| {
            let Some(run) = repo::get_run(conn, &lookup)? else {
                return Ok(None);
            };
            let events = repo::list_run_events(conn, &lookup)?;
            // Neighbours by time, not by recency, so an old run keeps its chain forever.
            let (from, to) = chain_bounds(&run.started_at, window);
            let recent = repo::list_runs_between(conn, &from, &to)?;
            Ok(Some((run, events, recent)))
        })
        .await;
    match loaded {
        Ok(Some((run, events, recent))) => {
            let turns = fold_turns(&events);
            let caps = caps_of(&state.config, &events);
            let chain: Vec<AttemptOutcome> = retry_chain(&run.id, &recent, window);
            Html(render_run(&run, &chain, &caps, &turns).into_string()).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => internal(e),
    }
}
