//! `/runs` and `/runs/{id}/log`: the run index and one run's event log, rendered from `runs` and
//! `run_events` (the same rows the runner writes as the harness streams).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use super::internal;
use crate::db::repo;
use crate::web::app::AppState;
use crate::web::views::log::{INDEX_LIMIT, render_index, render_log};

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
    let result = state
        .db
        .call(move |conn| {
            let Some(run) = repo::get_run(conn, &lookup)? else {
                return Ok(None);
            };
            let events = repo::list_run_events(conn, &lookup)?;
            Ok(Some((run, events)))
        })
        .await;
    match result {
        Ok(Some((run, events))) => Html(render_log(&run, &events).into_string()).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => internal(e),
    }
}
