//! Feed autodiscovery and validation for the Curator (`spec/m3.md` curator-tools). Every
//! request goes through the guarded [`Http`] (public addresses only, body cap, redirect
//! policy); a candidate URL on a private address is dropped before any request is made.

use std::collections::HashSet;

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use super::http::{Http, HttpError, check_url, get_text, read_capped};
use super::time::to_iso;

/// Paths probed on the page's origin when the page advertises nothing.
pub const PROBE_PATHS: [&str; 4] = ["/feed", "/rss.xml", "/atom.xml", "/index.xml"];
/// `itemsPerDay` is measured over this many days before `now`.
pub const RATE_DAYS: i64 = 30;
/// A page or feed larger than this is not a candidate (well under the ingest cap).
pub const MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("{0}")]
    Http(#[from] HttpError),
    #[error("not a feed: {0}")]
    NotAFeed(String),
}

/// A feed a page advertised or a probe found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundFeed {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `rss`, `atom`, `json`, or the `<link type>` when it was only advertised.
    #[serde(rename = "type")]
    pub kind: String,
}

/// What `validate_feed` reports for a feed that parses.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Validated {
    pub ok: bool,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub items: usize,
    pub items_per_day: f64,
    pub last_item_at: Option<String>,
}

fn kind_of(t: feed_rs::model::FeedType) -> &'static str {
    use feed_rs::model::FeedType;
    match t {
        FeedType::Atom => "atom",
        FeedType::JSON => "json",
        FeedType::RSS0 | FeedType::RSS1 | FeedType::RSS2 => "rss",
    }
}

/// Fetches and parses one feed: its title, type, entry count, items per day over the last
/// [`RATE_DAYS`] and the newest entry date. HTTP failures and non-feeds are typed errors.
pub async fn validate_feed(
    http: &Http,
    url: &str,
    now: DateTime<Utc>,
) -> Result<Validated, DiscoveryError> {
    let resp = http.get(url)?.send().await.map_err(HttpError::from)?;
    if !resp.status().is_success() {
        return Err(HttpError::Status(resp.status().as_u16()).into());
    }
    let bytes = read_capped(resp, MAX_BYTES).await?;
    let feed =
        feed_rs::parser::parse(&bytes[..]).map_err(|e| DiscoveryError::NotAFeed(e.to_string()))?;
    let since = now - Duration::days(RATE_DAYS);
    let dates: Vec<DateTime<Utc>> = feed
        .entries
        .iter()
        .filter_map(|e| e.published.or(e.updated))
        .collect();
    let recent = dates.iter().filter(|d| **d >= since && **d <= now).count();
    let items_per_day = (recent as f64 / RATE_DAYS as f64 * 100.0).round() / 100.0;
    Ok(Validated {
        ok: true,
        title: feed
            .title
            .map(|t| t.content.trim().to_string())
            .unwrap_or_default(),
        kind: kind_of(feed.feed_type).to_string(),
        items: feed.entries.len(),
        items_per_day,
        last_item_at: dates.iter().max().map(|d| to_iso(*d)),
    })
}

/// The `<link rel="alternate">` feeds a page advertises, resolved against `base`, in document
/// order without duplicates. A small scanner: attributes quoted with `"`, `'` or bare.
pub fn link_alternates(html: &str, base: &url::Url) -> Vec<FoundFeed> {
    let mut out: Vec<FoundFeed> = Vec::new();
    let mut seen = HashSet::new();
    let lower = html.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(start) = lower[pos..].find("<link") {
        let start = pos + start;
        let Some(end) = lower[start..].find('>') else {
            break;
        };
        let end = start + end;
        let tag = &html[start + 5..end];
        pos = end + 1;
        let attr = |name: &str| attribute(tag, name);
        let rel = attr("rel").unwrap_or_default().to_ascii_lowercase();
        let kind = attr("type").unwrap_or_default().to_ascii_lowercase();
        if !rel.split_whitespace().any(|r| r == "alternate") {
            continue;
        }
        let feed_kind = if kind.contains("rss") {
            "rss"
        } else if kind.contains("atom") {
            "atom"
        } else if kind.contains("feed+json") || kind.contains("json") {
            "json"
        } else {
            continue;
        };
        let Some(href) = attr("href") else { continue };
        let Ok(resolved) = base.join(href.trim()) else {
            continue;
        };
        let url = resolved.to_string();
        if seen.insert(url.clone()) {
            out.push(FoundFeed {
                url,
                title: attr("title")
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty()),
                kind: feed_kind.to_string(),
            });
        }
    }
    out
}

/// The value of `name="..."` / `name='...'` / `name=bare` inside one tag's attributes.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let i = from + i;
        let before_ok = i == 0 || !lower.as_bytes()[i - 1].is_ascii_alphanumeric();
        let rest = &tag[i + name.len()..];
        let rest_trim = rest.trim_start();
        if before_ok && rest_trim.starts_with('=') {
            let v = rest_trim[1..].trim_start();
            return Some(match v.chars().next() {
                Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or(""),
                // A bare value runs to whitespace or the tag end; a self-closing tag's `/`
                // is not part of it.
                _ => v
                    .split(|c: char| c.is_whitespace() || c == '>')
                    .next()
                    .unwrap_or("")
                    .trim_end_matches('/'),
            });
        }
        from = i + name.len();
    }
    None
}

/// Feeds for a page: what it advertises (private-address links dropped, no request made for
/// them) plus every [`PROBE_PATHS`] on the page's origin that validates as a feed.
pub async fn find_feeds(
    http: &Http,
    page_url: &str,
    now: DateTime<Utc>,
    allow_loopback: bool,
) -> Result<Vec<FoundFeed>, DiscoveryError> {
    let base = url::Url::parse(page_url).map_err(|_| HttpError::Scheme(page_url.to_string()))?;
    let html = get_text(http, page_url, MAX_BYTES).await?;
    let mut found: Vec<FoundFeed> = link_alternates(&html, &base)
        .into_iter()
        .filter(|f| check_url(&f.url, allow_loopback).is_ok())
        .collect();
    let Ok(origin) = base.join("/") else {
        return Ok(found);
    };
    for path in PROBE_PATHS {
        let Ok(candidate) = origin.join(path) else {
            continue;
        };
        let candidate = candidate.to_string();
        if found.iter().any(|f| f.url == candidate) {
            continue;
        }
        if let Ok(v) = validate_feed(http, &candidate, now).await {
            found.push(FoundFeed {
                url: candidate,
                title: Some(v.title).filter(|t| !t.is_empty()),
                kind: v.kind,
            });
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::http::client;
    use chrono::TimeZone;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 17, 6, 0, 0).unwrap()
    }

    fn http() -> Http {
        client(&crate::config::Ingest {
            concurrency: 2,
            request_timeout_ms: 2000,
            total_budget_ms: 10_000,
            body_max_bytes: 2 * 1024 * 1024,
            max_redirects: 3,
            dedupe_cosine: 0.92,
            allow_loopback: true,
        })
        .unwrap()
    }

    async fn mount(server: &MockServer, p: &str, template: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(template)
            .mount(server)
            .await;
    }

    /// RSS 2.0 with one item per date (RFC 2822 pubDate).
    fn rss(title: &str, dates: &[&str]) -> String {
        let items: String = dates
            .iter()
            .enumerate()
            .map(|(i, d)| format!("<item><title>Post {i}</title><link>https://x.example/{i}</link><pubDate>{d}</pubDate></item>"))
            .collect();
        format!(
            "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>{title}</title><link>https://x.example/</link><description>d</description>{items}</channel></rss>"
        )
    }

    fn atom(title: &str, dates: &[&str]) -> String {
        let entries: String = dates
            .iter()
            .enumerate()
            .map(|(i, d)| format!("<entry><title>Post {i}</title><id>urn:{i}</id><link href=\"https://x.example/{i}\"/><updated>{d}</updated></entry>"))
            .collect();
        format!(
            "<?xml version=\"1.0\"?><feed xmlns=\"http://www.w3.org/2005/Atom\"><title>{title}</title><id>urn:f</id><updated>2026-09-16T00:00:00Z</updated>{entries}</feed>"
        )
    }

    #[tokio::test]
    async fn validate_feed_reports_items_per_day() {
        let server = MockServer::start().await;
        // Three items in the last 30 days, one older, one dated in the future (ignored).
        mount(
            &server,
            "/rss",
            ResponseTemplate::new(200).set_body_string(rss(
                "The Blog",
                &[
                    "Wed, 16 Sep 2026 08:00:00 GMT",
                    "Tue, 01 Sep 2026 08:00:00 GMT",
                    "Sun, 30 Aug 2026 08:00:00 GMT",
                    "Sat, 01 Aug 2026 08:00:00 GMT",
                    "Fri, 01 Jan 2027 08:00:00 GMT",
                ],
            )),
        )
        .await;
        let v = validate_feed(&http(), &format!("{}/rss", server.uri()), now())
            .await
            .unwrap();
        assert_eq!(v.title, "The Blog");
        assert_eq!(v.kind, "rss");
        assert_eq!(v.items, 5);
        assert_eq!(v.items_per_day, 0.1, "3 items / 30 days");
        assert_eq!(v.last_item_at.as_deref(), Some("2027-01-01T08:00:00.000Z"));
        assert!(v.ok);
        mount(
            &server,
            "/atom",
            ResponseTemplate::new(200).set_body_string(atom(
                "Atom One",
                &["2026-09-16T09:00:00Z", "2026-09-10T09:00:00Z"],
            )),
        )
        .await;
        let v = validate_feed(&http(), &format!("{}/atom", server.uri()), now())
            .await
            .unwrap();
        assert_eq!(
            (v.kind.as_str(), v.title.as_str(), v.items),
            ("atom", "Atom One", 2)
        );
        assert_eq!(v.items_per_day, 0.07);
        assert_eq!(v.last_item_at.as_deref(), Some("2026-09-16T09:00:00.000Z"));
    }

    #[tokio::test]
    async fn validate_feed_rejects_non_feeds_and_404() {
        let server = MockServer::start().await;
        mount(
            &server,
            "/page",
            ResponseTemplate::new(200).set_body_string("<html><body>hello</body></html>"),
        )
        .await;
        mount(&server, "/gone", ResponseTemplate::new(404)).await;
        let err = validate_feed(&http(), &format!("{}/page", server.uri()), now())
            .await
            .unwrap_err();
        assert!(err.to_string().starts_with("not a feed: "), "{err}");
        let err = validate_feed(&http(), &format!("{}/gone", server.uri()), now())
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "HTTP 404");
        let err = validate_feed(&http(), "ftp://x.example/feed", now())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not http(s)"), "{err}");
        let err = validate_feed(&http(), "http://10.0.0.1/feed", now())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("private address"), "{err}");
    }

    #[test]
    fn link_alternates_parses_quoting_styles_and_resolves_relative_hrefs() {
        let base = url::Url::parse("https://blog.example/posts/").unwrap();
        let html = r#"<html><head>
            <LINK REL="alternate" TYPE="application/rss+xml" TITLE="Posts" HREF="/feed.xml">
            <link rel='alternate' type='application/atom+xml' href='atom.xml' title=''/>
            <link rel=alternate type=application/feed+json href=https://blog.example/feed.json>
            <link rel="stylesheet" href="/style.css">
            <link rel="alternate" type="text/html" hreflang="vi" href="/vi/">
            <link rel="alternate" type="application/rss+xml" href="/feed.xml">
            </head></html>"#;
        let got = link_alternates(html, &base);
        assert_eq!(
            got,
            vec![
                FoundFeed {
                    url: "https://blog.example/feed.xml".into(),
                    title: Some("Posts".into()),
                    kind: "rss".into()
                },
                FoundFeed {
                    url: "https://blog.example/posts/atom.xml".into(),
                    title: None,
                    kind: "atom".into()
                },
                FoundFeed {
                    url: "https://blog.example/feed.json".into(),
                    title: None,
                    kind: "json".into()
                },
            ]
        );
    }

    #[tokio::test]
    async fn find_feeds_discovers_link_tags_and_common_paths() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(&server, "/", ResponseTemplate::new(200).set_body_string(format!(
            r#"<html><head><title>Home</title>
            <link rel="alternate" type="application/rss+xml" title="Main" href="{u}/main.rss">
            <link rel="alternate" type="application/atom+xml" title="Comments" href="/comments.atom">
            </head><body>hi</body></html>"#
        ))).await;
        // One probe hits (a real feed), the others 404; the advertised feeds are not fetched.
        mount(
            &server,
            "/index.xml",
            ResponseTemplate::new(200)
                .set_body_string(rss("Index Feed", &["Wed, 16 Sep 2026 08:00:00 GMT"])),
        )
        .await;
        for p in ["/feed", "/rss.xml", "/atom.xml"] {
            mount(&server, p, ResponseTemplate::new(404)).await;
        }
        let got = find_feeds(&http(), &format!("{u}/"), now(), true)
            .await
            .unwrap();
        let urls: Vec<(&str, &str, Option<&str>)> = got
            .iter()
            .map(|f| (f.url.as_str(), f.kind.as_str(), f.title.as_deref()))
            .collect();
        assert_eq!(
            urls,
            vec![
                (format!("{u}/main.rss").as_str(), "rss", Some("Main")),
                (
                    format!("{u}/comments.atom").as_str(),
                    "atom",
                    Some("Comments")
                ),
                (format!("{u}/index.xml").as_str(), "rss", Some("Index Feed")),
            ]
        );
        let requests = server.received_requests().await.unwrap();
        let paths: Vec<String> = requests.iter().map(|r| r.url.path().to_string()).collect();
        assert!(
            !paths
                .iter()
                .any(|p| p == "/main.rss" || p == "/comments.atom")
        );
    }

    #[tokio::test]
    async fn find_feeds_drops_private_links_without_a_request() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(
            &server,
            "/",
            ResponseTemplate::new(200).set_body_string(
                r#"<html><head>
            <link rel="alternate" type="application/rss+xml" href="http://10.0.0.1/feed">
            <link rel="alternate" type="application/rss+xml" href="http://localhost/feed">
            </head></html>"#,
            ),
        )
        .await;
        for p in PROBE_PATHS {
            mount(&server, p, ResponseTemplate::new(404)).await;
        }
        // The policy of the deployed client: loopback links are private too (the page itself
        // is on loopback only because the test client allows it).
        let got = find_feeds(&http(), &format!("{u}/"), now(), false)
            .await
            .unwrap();
        assert!(got.is_empty(), "{got:?}");
        let n = server.received_requests().await.unwrap().len();
        assert_eq!(
            n,
            1 + PROBE_PATHS.len(),
            "the page and the four probes, nothing else"
        );
    }
}
