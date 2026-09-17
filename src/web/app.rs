//! Router, shared state, the bind guard and the security headers every response carries.

use axum::Router;
use axum::http::{HeaderValue, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use super::routes;
use crate::config::Config;
use crate::db::Db;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BindError {
    #[error("refusing to bind {0}: only 127.0.0.1 (or 0.0.0.0 inside the container) is allowed")]
    Refused(String),
}

/// Loopback on the host; `0.0.0.0` only inside the container, where compose publishes it on
/// `127.0.0.1:8788`.
pub fn assert_bind_allowed(bind: &str, in_container: bool) -> Result<(), BindError> {
    match bind {
        "127.0.0.1" | "localhost" | "::1" => Ok(()),
        "0.0.0.0" if in_container => Ok(()),
        other => Err(BindError::Refused(other.to_string())),
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Config,
    pub tz: Tz,
    pub now: fn() -> DateTime<Utc>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(routes::digest::today))
        .route("/d/{date}", get(routes::digest::day))
        .fallback(routes::not_found)
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

async fn security_headers(req: axum::extract::Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_guard_allows_loopback_and_container_only() {
        assert_eq!(assert_bind_allowed("127.0.0.1", false), Ok(()));
        assert_eq!(assert_bind_allowed("0.0.0.0", true), Ok(()));
        assert_eq!(
            assert_bind_allowed("0.0.0.0", false),
            Err(BindError::Refused("0.0.0.0".into()))
        );
        assert_eq!(
            assert_bind_allowed("192.168.1.5", true),
            Err(BindError::Refused("192.168.1.5".into()))
        );
    }
}
