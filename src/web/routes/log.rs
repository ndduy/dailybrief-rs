//! `/runs/{id}/log`: one run's raw event log, rendered from `run_events` (the same rows the
//! runner writes as the harness streams). The index and the turn view live in `routes::runs`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};

use super::internal;
use crate::db::repo;
use crate::web::app::AppState;
use crate::web::views::log::{EVENT_CAP, render_log};

pub async fn show(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let lookup = id.clone();
    let result = state
        .db
        .call(move |conn| {
            let Some(run) = repo::get_run(conn, &lookup)? else {
                return Ok(None);
            };
            let mut events = repo::list_run_events(conn, &lookup)?;
            let total = events.len();
            events.truncate(EVENT_CAP);
            Ok(Some((run, events, total)))
        })
        .await;
    match result {
        Ok(Some((run, events, total))) => {
            Html(render_log(&run, &events, total).into_string()).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Err(e) => internal(e),
    }
}
