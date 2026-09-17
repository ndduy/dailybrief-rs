//! `/runs/{id}/transcript`: the raw `stream-json` file of a run (M2 replaces it with a parsed page).

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::internal;
use crate::db::repo;
use crate::web::app::AppState;

pub async fn download(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let lookup = id.clone();
    let run = match state
        .db
        .call(move |conn| repo::get_run(conn, &lookup))
        .await
    {
        Err(e) => return internal(e),
        Ok(run) => run,
    };
    let Some(path) = run.and_then(|r| r.transcript_path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (
                    header::CONTENT_TYPE,
                    "application/x-ndjson; charset=utf-8".to_string(),
                ),
                (
                    header::CONTENT_DISPOSITION,
                    format!("inline; filename=\"{id}.jsonl\""),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "not found").into_response()
        }
        Err(e) => internal(e),
    }
}
