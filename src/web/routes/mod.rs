//! Route handlers. Each one maps a request onto repo calls inside one `Db::call` and a view.

pub mod digest;
pub mod log;
pub mod redirect;
pub mod run;
pub mod runs;
pub mod transcript;

use axum::http::StatusCode;
use axum::response::IntoResponse;

pub async fn not_found() -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "not found")
}

/// A database failure is a 500 with a fixed body; the detail goes to the log, never the page.
pub fn internal(e: impl std::fmt::Display) -> axum::response::Response {
    tracing::error!(error = %e, "request failed");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}
