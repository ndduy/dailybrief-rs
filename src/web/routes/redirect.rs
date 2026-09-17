//! `/r/{id}`: the success metric (`SPEC.md` §1 #10). Logs the click, then 302s to the article.

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::internal;
use crate::core::time::to_iso;
use crate::db::repo;
use crate::web::app::AppState;

pub async fn click(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let at = to_iso((state.now)());
    let lookup = id.clone();
    let target = state
        .db
        .call(move |conn| {
            let Some(item) = repo::get_item(conn, &lookup)? else {
                return Ok(None);
            };
            let digest_id = repo::latest_digest_for_item(conn, &lookup)?;
            repo::insert_read(conn, &lookup, digest_id.as_deref(), &at)?;
            Ok(Some(item.canonical_url))
        })
        .await;
    match target {
        Err(e) => internal(e),
        Ok(None) => (StatusCode::NOT_FOUND, "not found").into_response(),
        Ok(Some(url)) => (StatusCode::FOUND, [(header::LOCATION, url)]).into_response(),
    }
}
