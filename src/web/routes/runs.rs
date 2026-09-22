//! `/runs` (index) and `/runs/{id}` (one run as turns, caps and retry chain, ADR 0012).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use super::internal;
use crate::config::Config;
use crate::core::time::{parse_iso, to_iso};
use crate::db::repo::{self, RunEvent};
use crate::harness::trajectory::{
    AttemptOutcome, CapsUsed, Event, ResultEvent, Turn, caps_used, fold_turns, parse_line,
    retry_chain,
};
use crate::web::app::AppState;
use crate::web::views::run::{render_index, render_run};

/// Rows on the runs index.
pub const INDEX_LIMIT: i64 = 30;

/// The run's `result` line, if it has one.
pub fn result_of(events: &[RunEvent]) -> Option<ResultEvent> {
    events
        .iter()
        .rev()
        .find_map(|e| match parse_line(&e.payload_json) {
            Event::Result(r) => Some(r),
            _ => None,
        })
}

/// What a run consumed, from turns the caller folded once and the configured caps.
pub fn caps_of(config: &Config, turns: &[Turn], result: Option<&ResultEvent>) -> CapsUsed {
    caps_used(turns, result, &config.caps, &config.harness.claude_code)
}

/// Turns and caps for a page, folding the events exactly once.
pub fn page_data(config: &Config, events: &[RunEvent]) -> (Vec<Turn>, CapsUsed) {
    let turns = fold_turns(events);
    let caps = caps_of(config, &turns, result_of(events).as_ref());
    (turns, caps)
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
            let (turns, caps) = page_data(&state.config, &events);
            let chain: Vec<AttemptOutcome> = retry_chain(&run.id, &recent, window);
            Html(render_run(&run, &chain, &caps, &turns).into_string()).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Env, load_config};
    use crate::db::Db;
    use crate::db::repo::{NewRun, Role, RunKind};

    /// `caps_of` no longer folds for itself: handed no turns, it reports no turns even when
    /// the result line claims some, so a page that folds once cannot fold twice by accident.
    #[test]
    fn run_page_folds_once() {
        let config = load_config(&Env::from_lookup(|_| None).unwrap()).unwrap();
        let db = Db::open_in_memory().unwrap();
        let events = db
            .with(|c| {
                repo::insert_run(
                    c,
                    &NewRun {
                        id: "r".into(),
                        kind: RunKind::Manual,
                        role: Role::Editor,
                        harness: "test".into(),
                        attempt: 1,
                        started_at: "2026-09-17T06:00:00.000Z".into(),
                        transcript_path: None,
                    },
                )?;
                for i in 1..=3 {
                    repo::append_run_event(
                        c,
                        "r",
                        i,
                        "assistant",
                        &format!(
                            r#"{{"type":"assistant","message":{{"id":"m{i}","content":[{{"type":"tool_use","id":"t{i}","name":"mcp__dailybrief__read_item","input":{{}}}}]}}}}"#
                        ),
                    )?;
                }
                repo::append_run_event(
                    c,
                    "r",
                    4,
                    "result",
                    r#"{"type":"result","subtype":"success","num_turns":3,"duration_ms":1000}"#,
                )?;
                repo::list_run_events(c, "r")
            })
            .unwrap();
        let (turns, caps) = page_data(&config, &events);
        assert_eq!((turns.len(), caps.turns.used, caps.reads.used), (3, 3, 3));
        let unfolded = caps_of(&config, &[], result_of(&events).as_ref());
        assert_eq!((unfolded.turns.used, unfolded.reads.used), (0, 0));
    }
}
