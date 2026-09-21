//! One `reqwest` client for feeds and article pages (`SPEC.md` §4 `fetch_sources` limits):
//! rustls, gzip/brotli, per-request timeout, bounded redirects, http(s) only, streamed body cap,
//! and public addresses only: feed entries are third-party input, so loopback, link-local,
//! RFC 1918, CGNAT and unique-local targets are refused on the first hop, on every redirect
//! hop, and at DNS resolution (`allow_loopback` exists for tests against a local mock).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use crate::config::Ingest;

pub const USER_AGENT: &str = concat!("dailybrief/", env!("CARGO_PKG_VERSION"));

const PRIVATE_MSG: &str = "private address";

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("url '{0}' is not http(s)")]
    Scheme(String),
    #[error("host '{0}' is a private address")]
    PrivateAddress(String),
    #[error("too many redirects")]
    TooManyRedirects,
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
    let mentions_private = |e: &reqwest::Error| {
        let mut cause = std::error::Error::source(e);
        while let Some(c) = cause {
            if c.to_string().contains(PRIVATE_MSG) {
                return true;
            }
            cause = c.source();
        }
        false
    };
    if e.is_timeout() {
        "timed out".to_string()
    } else if e.is_redirect() {
        if mentions_private(e) {
            "redirect to a private address".to_string()
        } else {
            "too many redirects".to_string()
        }
    } else if e.is_connect() {
        if mentions_private(e) {
            PRIVATE_MSG.to_string()
        } else {
            "connection failed".to_string()
        }
    } else if e.is_builder() {
        "invalid request".to_string()
    } else {
        let s = e.to_string();
        s.split(": ").next().unwrap_or(&s).to_string()
    }
}

fn private_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || o[0] == 0
        || (o[0] == 100 && (64..=127).contains(&o[1])) // CGNAT 100.64/10
        || o[0] >= 240 // reserved
}

fn private_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return private_v4(v4);
    }
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_multicast()
}

/// Whether an address may be fetched: public only, or loopback too when `allow_loopback`.
pub fn address_allowed(ip: IpAddr, allow_loopback: bool) -> bool {
    if allow_loopback && ip.is_loopback() {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => !private_v4(v4),
        IpAddr::V6(v6) => !private_v6(v6),
    }
}

/// http(s) only, and no literal private address or `localhost` name. Names that resolve to
/// private addresses are refused by [`GuardedResolver`] at connection time.
pub fn check_url(url: &str, allow_loopback: bool) -> Result<(), HttpError> {
    let Ok(u) = url::Url::parse(url) else {
        return Err(HttpError::Scheme(url.to_string()));
    };
    if !matches!(u.scheme(), "http" | "https") {
        return Err(HttpError::Scheme(url.to_string()));
    }
    match u.host() {
        Some(url::Host::Ipv4(ip)) if !address_allowed(ip.into(), allow_loopback) => {
            Err(HttpError::PrivateAddress(ip.to_string()))
        }
        Some(url::Host::Ipv6(ip)) if !address_allowed(ip.into(), allow_loopback) => {
            Err(HttpError::PrivateAddress(ip.to_string()))
        }
        Some(url::Host::Domain(d))
            if !allow_loopback
                && (d.eq_ignore_ascii_case("localhost")
                    || d.to_ascii_lowercase().ends_with(".localhost")) =>
        {
            Err(HttpError::PrivateAddress(d.to_string()))
        }
        Some(_) => Ok(()),
        None => Err(HttpError::Scheme(url.to_string())),
    }
}

/// System DNS, refusing any name that resolves to a private address (a mixed answer counts
/// as private: that is what a rebinding attempt looks like).
struct GuardedResolver {
    allow_loopback: bool,
}

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let allow = self.allow_loopback;
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if addrs.iter().any(|a| !address_allowed(a.ip(), allow)) {
                return Err(std::io::Error::other(PRIVATE_MSG).into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// The shared client plus its address policy; every request goes through [`Http::get`].
#[derive(Clone)]
pub struct Http {
    inner: reqwest::Client,
    allow_loopback: bool,
}

impl Http {
    /// A GET request builder, after the URL check.
    pub fn get(&self, url: &str) -> Result<reqwest::RequestBuilder, HttpError> {
        check_url(url, self.allow_loopback)?;
        Ok(self.inner.get(url))
    }
}

/// Builds the shared client from the `[ingest]` settings.
pub fn client(ingest: &Ingest) -> Result<Http, HttpError> {
    let max = ingest.max_redirects as usize;
    let allow_loopback = ingest.allow_loopback;
    let inner = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_millis(ingest.request_timeout_ms))
        .connect_timeout(Duration::from_millis(ingest.request_timeout_ms.min(10_000)))
        .dns_resolver(GuardedResolver { allow_loopback })
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() > max {
                attempt.error(HttpError::TooManyRedirects)
            } else if let Err(e) = check_url(attempt.url().as_str(), allow_loopback) {
                attempt.error(e)
            } else {
                attempt.follow()
            }
        }))
        .build()?;
    Ok(Http {
        inner,
        allow_loopback,
    })
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
pub async fn get_text(client: &Http, url: &str, max: u64) -> Result<String, HttpError> {
    let resp = client.get(url)?.send().await?;
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
            allow_loopback: true,
        }
    }

    #[test]
    fn private_ranges_are_refused_and_public_addresses_allowed() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.255",
            "0.0.0.0",
            "255.255.255.255",
            "240.0.0.1",
            "::1",
            "::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "::ffff:127.0.0.1",
        ] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(!address_allowed(ip, false), "{ip} must be refused");
        }
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "2606:4700::1111",
        ] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(address_allowed(ip, false), "{ip} must be allowed");
        }
        assert!(address_allowed("127.0.0.1".parse().unwrap(), true));
        assert!(address_allowed("::1".parse().unwrap(), true));
        assert!(!address_allowed("10.0.0.1".parse().unwrap(), true));
    }

    #[test]
    fn check_url_refuses_literal_private_hosts_and_localhost_names() {
        assert!(matches!(
            check_url("http://172.17.0.1:9000/admin", false),
            Err(HttpError::PrivateAddress(h)) if h == "172.17.0.1"
        ));
        assert!(matches!(
            check_url("http://[::1]/", false),
            Err(HttpError::PrivateAddress(_))
        ));
        assert!(matches!(
            check_url("http://localhost:8788/", false),
            Err(HttpError::PrivateAddress(_))
        ));
        assert!(matches!(
            check_url("http://foo.localhost/", false),
            Err(HttpError::PrivateAddress(_))
        ));
        assert!(check_url("http://127.0.0.1:1/", true).is_ok());
        assert!(check_url("https://example.com/a", false).is_ok());
        assert!(matches!(
            check_url("ftp://example.com/a", false),
            Err(HttpError::Scheme(_))
        ));
    }

    /// The production policy against a local mock: refused before any request is sent.
    #[tokio::test]
    async fn loopback_target_is_refused_without_a_request_when_not_allowed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("secret"))
            .mount(&server)
            .await;
        let mut cfg = ingest(1000, 3);
        cfg.allow_loopback = false;
        let c = client(&cfg).unwrap();
        let err = get_text(&c, &format!("{}/admin", server.uri()), 1024)
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::PrivateAddress(_)), "{err}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// A redirect hop is checked like a first hop: a public-looking start that bounces into
    /// RFC 1918 space is refused, while a hop to an allowed address is followed.
    #[tokio::test]
    async fn redirect_into_private_space_is_refused_on_the_hop() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/bounce"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "http://10.255.255.1/x"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/hop"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/ok", server.uri())),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/ok"))
            .respond_with(ResponseTemplate::new(200).set_body_string("fine"))
            .mount(&server)
            .await;
        let c = client(&ingest(2000, 3)).unwrap();
        let err = get_text(&c, &format!("{}/bounce", server.uri()), 1024)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "redirect to a private address");
        let body = get_text(&c, &format!("{}/hop", server.uri()), 1024)
            .await
            .unwrap();
        assert_eq!(body, "fine");
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
