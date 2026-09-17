//! One `reqwest` client for feeds and article pages (`SPEC.md` §4 `fetch_sources` limits):
//! rustls, gzip/brotli, per-request timeout, bounded redirects, http(s) only, streamed body cap.

use std::time::Duration;

use futures_util::StreamExt;

use crate::config::Ingest;

pub const USER_AGENT: &str = concat!("dailybrief/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("url '{0}' is not http(s)")]
    Scheme(String),
    #[error("body exceeds {max} bytes")]
    TooLarge { max: u64 },
    #[error("HTTP {0}")]
    Status(u16),
    #[error("{0}")]
    Request(String),
}

impl From<reqwest::Error> for HttpError {
    fn from(e: reqwest::Error) -> Self {
        Self::Request(fetch_error_message(&e))
    }
}

/// A short, stable message for a `reqwest` failure (no URL echo, no nested causes).
pub fn fetch_error_message(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".to_string()
    } else if e.is_redirect() {
        "too many redirects".to_string()
    } else if e.is_connect() {
        "connection failed".to_string()
    } else if e.is_builder() {
        "invalid request".to_string()
    } else {
        let s = e.to_string();
        s.split(": ").next().unwrap_or(&s).to_string()
    }
}

/// Builds the shared client from the `[ingest]` settings.
pub fn client(ingest: &Ingest) -> Result<reqwest::Client, HttpError> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_millis(ingest.request_timeout_ms))
        .connect_timeout(Duration::from_millis(ingest.request_timeout_ms.min(10_000)))
        .redirect(reqwest::redirect::Policy::limited(
            ingest.max_redirects as usize,
        ))
        .build()?;
    Ok(client)
}

/// Rejects anything but http(s) before a request is built.
pub fn check_scheme(url: &str) -> Result<(), HttpError> {
    match url::Url::parse(url) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => Ok(()),
        _ => Err(HttpError::Scheme(url.to_string())),
    }
}

/// Streams the body, failing as soon as it passes `max` bytes.
pub async fn read_capped(resp: reqwest::Response, max: u64) -> Result<Vec<u8>, HttpError> {
    if resp.content_length().is_some_and(|len| len > max) {
        return Err(HttpError::TooLarge { max });
    }
    let mut out = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if (out.len() as u64 + chunk.len() as u64) > max {
            return Err(HttpError::TooLarge { max });
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// GET a page body as text (lossy UTF-8), with the scheme check, status check and body cap.
pub async fn get_text(client: &reqwest::Client, url: &str, max: u64) -> Result<String, HttpError> {
    check_scheme(url)?;
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(HttpError::Status(resp.status().as_u16()));
    }
    let bytes = read_capped(resp, max).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ingest(timeout_ms: u64, max_redirects: u32) -> Ingest {
        Ingest {
            concurrency: 2,
            request_timeout_ms: timeout_ms,
            total_budget_ms: 10_000,
            body_max_bytes: 2 * 1024 * 1024,
            max_redirects,
            dedupe_cosine: 0.92,
        }
    }

    #[tokio::test]
    async fn get_text_returns_body_and_sends_user_agent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/page"))
            .and(wiremock::matchers::header("user-agent", USER_AGENT))
            .respond_with(ResponseTemplate::new(200).set_body_string("<p>hi</p>"))
            .mount(&server)
            .await;
        let c = client(&ingest(1000, 3)).unwrap();
        let body = get_text(&c, &format!("{}/page", server.uri()), 1024)
            .await
            .unwrap();
        assert_eq!(body, "<p>hi</p>");
    }

    #[tokio::test]
    async fn errors_past_body_cap() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/big"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 3 * 1024 * 1024]))
            .mount(&server)
            .await;
        let c = client(&ingest(5000, 3)).unwrap();
        let err = get_text(&c, &format!("{}/big", server.uri()), 2 * 1024 * 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::TooLarge { .. }), "{err}");
    }

    #[tokio::test]
    async fn stops_after_max_redirects() {
        let server = MockServer::start().await;
        for i in 0..5 {
            Mock::given(method("GET"))
                .and(path(format!("/r{i}")))
                .respond_with(
                    ResponseTemplate::new(302).insert_header("location", format!("/r{}", i + 1)),
                )
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/r5"))
            .respond_with(ResponseTemplate::new(200).set_body_string("end"))
            .mount(&server)
            .await;
        let c = client(&ingest(5000, 3)).unwrap();
        let err = get_text(&c, &format!("{}/r0", server.uri()), 1024)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "too many redirects");
        let c = client(&ingest(5000, 6)).unwrap();
        assert_eq!(
            get_text(&c, &format!("{}/r0", server.uri()), 1024)
                .await
                .unwrap(),
            "end"
        );
    }

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let c = client(&ingest(1000, 3)).unwrap();
        let err = get_text(&c, "ftp://x.example/a", 1024).await.unwrap_err();
        assert!(matches!(err, HttpError::Scheme(_)), "{err}");
        let err = get_text(&c, "file:///etc/passwd", 1024).await.unwrap_err();
        assert!(matches!(err, HttpError::Scheme(_)), "{err}");
    }

    #[tokio::test]
    async fn times_out() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(800)))
            .mount(&server)
            .await;
        let c = client(&ingest(150, 3)).unwrap();
        let err = get_text(&c, &format!("{}/slow", server.uri()), 1024)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "timed out");
    }

    #[tokio::test]
    async fn non_success_status_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/gone"))
            .respond_with(ResponseTemplate::new(410))
            .mount(&server)
            .await;
        let c = client(&ingest(1000, 3)).unwrap();
        let err = get_text(&c, &format!("{}/gone", server.uri()), 1024)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "HTTP 410");
    }
}
