//! `/` and `/d/{date}`: the latest digest for the local day, else the day's latest run state.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::internal;
use crate::core::time::{date_in_zone, day_bounds_utc, parse_date};
use crate::db::repo::{self, RunStatus};
use crate::web::app::AppState;
use crate::web::views;

pub async fn today(State(state): State<AppState>) -> Response {
    let date = date_in_zone((state.now)(), state.tz);
    render_day(&state, &date).await
}

pub async fn day(State(state): State<AppState>, Path(date): Path<String>) -> Response {
    if parse_date(&date).is_none() {
        return (StatusCode::BAD_REQUEST, views::state::bad_date(&date)).into_response();
    }
    render_day(&state, &date).await
}

async fn render_day(state: &AppState, date: &str) -> Response {
    let Some((from, to)) = day_bounds_utc(date, state.tz) else {
        return (StatusCode::BAD_REQUEST, views::state::bad_date(date)).into_response();
    };
    let d = date.to_string();
    let loaded = state
        .db
        .call(move |conn| {
            if let Some(digest) = repo::latest_digest_for_date(conn, &d)? {
                let cards = repo::list_digest_cards(conn, &digest.id)?;
                return Ok((Some((digest, cards)), None));
            }
            Ok((None, repo::latest_run_between(conn, &from, &to)?))
        })
        .await;
    match loaded {
        Err(e) => internal(e),
        Ok((Some((digest, cards)), _)) => views::digest::render(&digest, &cards).into_response(),
        Ok((None, Some(run))) => match run.status {
            RunStatus::Running => views::state::running(date, &run).into_response(),
            RunStatus::Failed | RunStatus::Killed | RunStatus::Success => {
                views::state::failed(date, &run).into_response()
            }
        },
        Ok((None, None)) => views::state::none(date).into_response(),
    }
}
