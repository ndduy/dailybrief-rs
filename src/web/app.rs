//! Router, shared state, the bind guard and the security headers every response carries.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use axum::Router;
use axum::http::{HeaderValue, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use super::auth::{SharedVerifier, require_access};
use super::routes;
use crate::config::Config;
use crate::db::Db;
use crate::harness::service_runner::ServiceRunner;

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
    /// `None` disables `POST /run` (503); the scheduler shares the same runner.
    pub runner: Option<Arc<ServiceRunner>>,
    /// One manual run in flight per process; the database lock covers other processes.
    pub active: Arc<AtomicBool>,
    /// `None` is the loopback bypass (`CF_ACCESS_AUD` unset on 127.0.0.1).
    pub access: SharedVerifier,
}

impl AppState {
    pub fn new(
        db: Db,
        config: Config,
        tz: Tz,
        now: fn() -> DateTime<Utc>,
        runner: Option<Arc<ServiceRunner>>,
    ) -> Self {
        Self {
            db,
            config,
            tz,
            now,
            runner,
            active: Arc::new(AtomicBool::new(false)),
            access: None,
        }
    }

    pub fn with_access(mut self, access: SharedVerifier) -> Self {
        self.access = access;
        self
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(routes::digest::today))
        .route("/d/{date}", get(routes::digest::day))
        .route("/r/{id}", get(routes::redirect::click))
        .route("/rate", post(routes::rate::rate))
        .route("/rate/reasons", get(routes::rate::reasons))
        .route("/run", post(routes::run::start))
        .route("/run/status", get(routes::run::status))
        .route("/runs", get(routes::runs::index))
        .route("/runs/{id}", get(routes::runs::show))
        .route("/runs/{id}/log", get(routes::log::show))
        .route("/runs/{id}/transcript", get(routes::transcript::download))
        .fallback(routes::not_found)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_access,
        ))
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
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// Scripts only from cdnjs (with SRI in the page), inline CSS, htmx requests to this origin,
/// forms to this origin, nothing framed. No `unsafe-eval`: the page uses no `hx-on`.
pub const CSP: &str = "default-src 'none'; script-src https://cdnjs.cloudflare.com; \
                       style-src 'unsafe-inline'; connect-src 'self'; form-action 'self'; \
                       base-uri 'none'; frame-ancestors 'none'";

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
