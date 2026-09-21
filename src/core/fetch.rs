//! Fetching one feed with a conditional GET and parsing it into entries (`feed-rs`).

use chrono::{DateTime, Utc};

use super::http::{Http, HttpError, read_capped};

/// One feed entry worth ingesting: it has a link and a title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub link: String,
    pub title: String,
    pub author: Option<String>,
    pub published: Option<DateTime<Utc>>,
    /// Summary or content body as the feed gave it (may be HTML); a fallback when the page fails.
    pub summary: Option<String>,
}

#[derive(Debug)]
pub enum FeedFetch {
    /// 304: the validators still match; nothing new.
    NotModified,
    /// A parsed feed plus the validators to store for next time.
    Entries {
        entries: Vec<Entry>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
    /// A short, stable reason (goes into `sources.last_error`).
    Failed(String),
}

/// GET the feed with `If-None-Match` / `If-Modified-Since` and parse it.
pub async fn fetch_feed(
    client: &Http,
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
    body_max_bytes: u64,
) -> FeedFetch {
    match fetch_feed_inner(client, url, etag, last_modified, body_max_bytes).await {
        Ok(f) => f,
        Err(e) => FeedFetch::Failed(e.to_string()),
    }
}

async fn fetch_feed_inner(
    client: &Http,
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
    body_max_bytes: u64,
) -> Result<FeedFetch, HttpError> {
    let mut req = client.get(url)?;
    if let Some(e) = etag {
        req = req.header(reqwest::header::IF_NONE_MATCH, e);
    }
    if let Some(lm) = last_modified {
        req = req.header(reqwest::header::IF_MODIFIED_SINCE, lm);
    }
    let resp = req.send().await?;
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(FeedFetch::NotModified);
    }
    if !resp.status().is_success() {
        return Err(HttpError::Status(resp.status().as_u16()));
    }
    let header = |name: reqwest::header::HeaderName| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let etag = header(reqwest::header::ETAG);
    let last_modified = header(reqwest::header::LAST_MODIFIED);
    let bytes = read_capped(resp, body_max_bytes).await?;
    let entries =
        parse_entries(&bytes).map_err(|e| HttpError::Request(format!("feed parse: {e}")))?;
    Ok(FeedFetch::Entries {
        entries,
        etag,
        last_modified,
    })
}

/// RSS `<author>` holds an email and `feed-rs` fills the name with the placeholder "author";
/// prefer a real name, else the email, else nothing.
fn person_name(p: &feed_rs::model::Person) -> Option<String> {
    let name = p.name.trim();
    if !name.is_empty() && !name.eq_ignore_ascii_case("author") {
        return Some(name.to_string());
    }
    p.email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
}

/// Parses RSS/Atom/JSON Feed bytes; entries without a link or a title are dropped.
pub fn parse_entries(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let feed = feed_rs::parser::parse(bytes).map_err(|e| e.to_string())?;
    Ok(feed
        .entries
        .into_iter()
        .filter_map(|e| {
            let link = e
                .links
                .iter()
                .find(|l| l.rel.as_deref().is_none_or(|r| r == "alternate"))
                .or(e.links.first())
                .map(|l| l.href.trim().to_string())
                .filter(|h| !h.is_empty())?;
            let title = e
                .title
                .map(|t| t.content.trim().to_string())
                .filter(|t| !t.is_empty())?;
            Some(Entry {
                link,
                title,
                author: e.authors.first().and_then(person_name),
                published: e.published.or(e.updated),
                summary: e
                    .summary
                    .map(|t| t.content)
                    .or_else(|| e.content.and_then(|c| c.body))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::http::client;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const RSS2: &str = include_str!("../../tests/fixtures/feeds/rss2.xml");
    const ATOM: &str = include_str!("../../tests/fixtures/feeds/atom.xml");

    fn test_client() -> Http {
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

    #[test]
    fn parses_rss2_fixture() {
        let entries = parse_entries(RSS2.as_bytes()).unwrap();
        let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(
            titles,
            vec![
                "Ownership in practice",
                "Async without the ceremony",
                "No date on this one"
            ],
            "entries without link or title are dropped"
        );
        assert_eq!(
            entries[0].link,
            "https://blog.example/posts/ownership?utm_source=rss"
        );
        assert_eq!(entries[0].author.as_deref(), Some("ana@example"));
        assert_eq!(
            entries[0].published.map(|d| d.to_rfc3339()),
            Some("2026-09-15T08:00:00+00:00".into())
        );
        assert_eq!(
            entries[0].summary.as_deref(),
            Some("Borrowing rules explained with three examples.")
        );
        assert!(
            entries[1]
                .summary
                .as_deref()
                .unwrap()
                .contains("<b>short</b>")
        );
        assert!(entries[2].published.is_none());
    }

    #[test]
    fn parses_atom_fixture() {
        let entries = parse_entries(ATOM.as_bytes()).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].link, "https://atom.example/type-driven");
        assert_eq!(entries[0].author.as_deref(), Some("Bình Nguyễn"));
        assert_eq!(
            entries[0].published.map(|d| d.to_rfc3339()),
            Some("2026-09-16T09:00:00+00:00".into())
        );
        assert_eq!(
            entries[1].published.map(|d| d.to_rfc3339()),
            Some("2026-09-15T10:00:00+00:00".into()),
            "falls back to updated"
        );
        assert_eq!(
            entries[1].summary.as_deref(),
            Some("<p>Body as content.</p>")
        );
        assert!(entries[2].published.is_none());
    }

    #[test]
    fn garbage_is_a_parse_error() {
        assert!(parse_entries(b"not xml at all").is_err());
    }

    #[tokio::test]
    async fn fetch_feed_sends_conditional_headers_and_returns_validators() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rss"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("etag", "W/\"new\"")
                    .insert_header("last-modified", "Tue, 15 Sep 2026 00:00:00 GMT")
                    .set_body_string(RSS2),
            )
            .mount(&server)
            .await;
        let got = fetch_feed(
            &test_client(),
            &format!("{}/rss", server.uri()),
            Some("W/\"old\""),
            Some("Mon, 14 Sep 2026 00:00:00 GMT"),
            1 << 20,
        )
        .await;
        let sent = &server.received_requests().await.unwrap()[0].headers;
        assert_eq!(sent.get("if-none-match").unwrap(), "W/\"old\"");
        assert_eq!(
            sent.get("if-modified-since").unwrap(),
            "Mon, 14 Sep 2026 00:00:00 GMT"
        );
        match got {
            FeedFetch::Entries {
                entries,
                etag,
                last_modified,
            } => {
                assert_eq!(entries.len(), 3);
                assert_eq!(etag.as_deref(), Some("W/\"new\""));
                assert_eq!(
                    last_modified.as_deref(),
                    Some("Tue, 15 Sep 2026 00:00:00 GMT")
                );
            }
            other => panic!("expected entries, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_feed_returns_not_modified_on_304() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rss"))
            .respond_with(ResponseTemplate::new(304))
            .mount(&server)
            .await;
        let got = fetch_feed(
            &test_client(),
            &format!("{}/rss", server.uri()),
            Some("x"),
            None,
            1 << 20,
        )
        .await;
        assert!(matches!(got, FeedFetch::NotModified), "{got:?}");
    }

    #[tokio::test]
    async fn fetch_feed_reports_failures_as_short_messages() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rss"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let got = fetch_feed(
            &test_client(),
            &format!("{}/rss", server.uri()),
            None,
            None,
            1 << 20,
        )
        .await;
        assert!(
            matches!(&got, FeedFetch::Failed(m) if m == "HTTP 500"),
            "{got:?}"
        );
        let got = fetch_feed(
            &test_client(),
            "gopher://x.example/rss",
            None,
            None,
            1 << 20,
        )
        .await;
        assert!(
            matches!(&got, FeedFetch::Failed(m) if m.contains("not http(s)")),
            "{got:?}"
        );
        let big = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rss"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 4096]))
            .mount(&big)
            .await;
        let got = fetch_feed(
            &test_client(),
            &format!("{}/rss", big.uri()),
            None,
            None,
            1024,
        )
        .await;
        assert!(
            matches!(&got, FeedFetch::Failed(m) if m.contains("exceeds 1024 bytes")),
            "{got:?}"
        );
    }
}
