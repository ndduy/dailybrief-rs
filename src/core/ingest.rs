//! The ingest pipeline behind `fetch_sources` and `dailybrief fetch` (`SPEC.md` §4): for every
//! enabled feed, fetch → for each new entry fetch the page → extract → dedupe → embed → store.
//! Feeds run concurrently under a total time budget; a feed that finishes keeps its items even
//! when the budget cuts the rest short.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::dedupe::{
    Duplicate, canonical_url, content_hash, find_duplicate_against, item_id, title_hash,
};
use super::embed::Embedder;
use super::extract::extract;
use super::fetch::{Entry, FeedFetch, fetch_feed};
use super::http::get_text;
use super::time::{days_ago_iso, to_iso};
use crate::config::{Config, Feed};
use crate::core::http::Http;
use crate::db::{Db, DbError, repo};

/// How much of an article the embedding sees (characters); BGE-small truncates at 512 tokens anyway.
const EMBED_CHARS: usize = 2000;
/// Near-duplicate window (days) for the cosine check.
const DEDUPE_DAYS: u32 = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedStatus {
    Ok,
    NotModified,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedReport {
    pub id: String,
    pub source: String,
    pub status: FeedStatus,
    pub new_items: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IngestReport {
    /// Feeds attempted.
    pub fetched: usize,
    pub new_items: usize,
    pub per_feed: Vec<FeedReport>,
}

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("http client: {0}")]
    Http(#[from] super::http::HttpError),
}

/// Everything one ingest run needs; cheap to clone into per-feed tasks.
#[derive(Clone)]
pub struct Ingester {
    pub db: Db,
    pub config: Config,
    pub client: Http,
    pub embedder: Arc<dyn Embedder>,
    pub now: fn() -> DateTime<Utc>,
}

impl Ingester {
    /// Mirrors `feeds` into `sources`, then ingests every enabled one under the time budget.
    pub async fn run(&self, feeds: &[Feed]) -> Result<IngestReport, IngestError> {
        {
            let feeds = feeds.to_vec();
            self.db
                .call(move |conn| {
                    for f in &feeds {
                        repo::upsert_source(conn, f)?;
                    }
                    Ok(())
                })
                .await?;
        }
        let enabled: Vec<Feed> = feeds.iter().filter(|f| f.enabled).cloned().collect();
        let permits = Arc::new(Semaphore::new(
            self.config.ingest.concurrency.max(1) as usize
        ));
        let mut set = JoinSet::new();
        for feed in enabled.clone() {
            let me = self.clone();
            let permits = Arc::clone(&permits);
            set.spawn(async move {
                let _permit = permits.acquire_owned().await;
                me.ingest_feed(&feed).await
            });
        }
        let budget = Duration::from_millis(self.config.ingest.total_budget_ms);
        let mut done: Vec<FeedReport> = Vec::new();
        let deadline = tokio::time::sleep(budget);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                next = set.join_next() => match next {
                    Some(Ok(report)) => done.push(report),
                    Some(Err(e)) => done.push(FeedReport {
                        id: "?".into(),
                        source: "?".into(),
                        status: FeedStatus::Failed,
                        new_items: 0,
                        error: Some(format!("task failed: {e}")),
                    }),
                    None => break,
                },
                () = &mut deadline => {
                    set.abort_all();
                    break;
                }
            }
        }
        // Feeds the budget cut off are reported as failed and their failure counted.
        for feed in &enabled {
            if !done.iter().any(|r| r.id == feed.id) {
                let id = feed.id.clone();
                self.db
                    .call(move |conn| repo::mark_source_failed(conn, &id, "ingest budget exceeded"))
                    .await?;
                done.push(FeedReport {
                    id: feed.id.clone(),
                    source: feed.title.clone(),
                    status: FeedStatus::Failed,
                    new_items: 0,
                    error: Some("ingest budget exceeded".into()),
                });
            }
        }
        done.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(IngestReport {
            fetched: enabled.len(),
            new_items: done.iter().map(|r| r.new_items).sum(),
            per_feed: done,
        })
    }

    async fn ingest_feed(&self, feed: &Feed) -> FeedReport {
        let report = |status, new_items, error: Option<String>| FeedReport {
            id: feed.id.clone(),
            source: feed.title.clone(),
            status,
            new_items,
            error,
        };
        match self.ingest_feed_inner(feed).await {
            Ok(FeedOutcome::NotModified) => report(FeedStatus::NotModified, 0, None),
            Ok(FeedOutcome::Stored(n)) => report(FeedStatus::Ok, n, None),
            Ok(FeedOutcome::Failed(msg)) => report(FeedStatus::Failed, 0, Some(msg)),
            Err(e) => report(FeedStatus::Failed, 0, Some(e.to_string())),
        }
    }

    async fn ingest_feed_inner(&self, feed: &Feed) -> Result<FeedOutcome, IngestError> {
        let id = feed.id.clone();
        let source = self
            .db
            .call(move |conn| repo::get_source(conn, &id))
            .await?;
        let (etag, last_modified) = source
            .map(|s| (s.etag, s.last_modified))
            .unwrap_or_default();
        let fetched = fetch_feed(
            &self.client,
            &feed.url,
            etag.as_deref(),
            last_modified.as_deref(),
            self.config.ingest.body_max_bytes,
        )
        .await;
        let now = (self.now)();
        match fetched {
            FeedFetch::NotModified => {
                let id = feed.id.clone();
                let at = to_iso(now);
                self.db
                    .call(move |conn| {
                        repo::mark_source_ok(
                            conn,
                            &id,
                            etag.as_deref(),
                            last_modified.as_deref(),
                            &at,
                        )
                    })
                    .await?;
                Ok(FeedOutcome::NotModified)
            }
            FeedFetch::Failed(msg) => {
                let id = feed.id.clone();
                let m = msg.clone();
                self.db
                    .call(move |conn| repo::mark_source_failed(conn, &id, &m))
                    .await?;
                Ok(FeedOutcome::Failed(msg))
            }
            FeedFetch::Entries {
                entries,
                etag,
                last_modified,
            } => {
                let stored = self.store_entries(feed, entries, now).await?;
                let id = feed.id.clone();
                let at = to_iso(now);
                self.db
                    .call(move |conn| {
                        repo::mark_source_ok(
                            conn,
                            &id,
                            etag.as_deref(),
                            last_modified.as_deref(),
                            &at,
                        )
                    })
                    .await?;
                Ok(FeedOutcome::Stored(stored))
            }
        }
    }

    /// Fetch, extract and embed every unseen entry, then dedupe-and-insert each one.
    async fn store_entries(
        &self,
        feed: &Feed,
        entries: Vec<Entry>,
        now: DateTime<Utc>,
    ) -> Result<usize, IngestError> {
        let mut pending: Vec<repo::NewItem> = Vec::new();
        let since = days_ago_iso(now, DEDUPE_DAYS);
        for entry in entries {
            let canonical = canonical_url(&entry.link);
            let th = title_hash(&entry.title);
            let (c, t, window) = (canonical.clone(), th.clone(), since.clone());
            let seen = self
                .db
                .call(move |conn| {
                    Ok(repo::has_canonical_url(conn, &c)?
                        || repo::has_title_hash_since(conn, &t, &window)?)
                })
                .await?;
            if seen {
                continue;
            }
            let Ok(html) =
                get_text(&self.client, &entry.link, self.config.ingest.body_max_bytes).await
            else {
                continue;
            };
            // dom_smoothie is CPU-bound on up to 2 MiB of HTML: off the async runtime
            // (CONSTRAINTS.md), and a panic inside the parser only skips this entry.
            let link = entry.link.clone();
            let Ok(Some(article)) =
                tokio::task::spawn_blocking(move || extract(&html, Some(&link))).await
            else {
                continue;
            };
            // The feed's title wins: entries without one were dropped by the parser.
            let title = entry.title.clone();
            pending.push(repo::NewItem {
                id: item_id(&canonical),
                source_id: feed.id.clone(),
                url: entry.link.clone(),
                canonical_url: canonical,
                title,
                author: entry.author.clone().or(article.byline),
                published_at: entry.published.map(to_iso),
                fetched_at: to_iso(now),
                word_count: article.word_count as i64,
                content_hash: content_hash(&article.text),
                title_hash: th,
                text: article.text,
                vector: None,
            });
        }
        if pending.is_empty() {
            return Ok(0);
        }
        // Embed outside the DB lock (ADR 0002), in one batch per feed.
        let texts: Vec<String> = pending
            .iter()
            .map(|i| embed_text(&i.title, &i.text))
            .collect();
        let embedder = Arc::clone(&self.embedder);
        let vectors = tokio::task::spawn_blocking(move || embedder.embed(&texts))
            .await
            .map_err(|_| DbError::Panicked)?
            .map_err(|e| IngestError::Db(DbError::Corrupt(format!("embedding failed: {e}"))))?;
        for (item, v) in pending.iter_mut().zip(vectors) {
            item.vector = Some(v);
        }
        let threshold = self.config.ingest.dedupe_cosine;
        let stored = self
            .db
            .call(move |conn| {
                // The `(id, vector)` projection is loaded once per batch (not once per item)
                // and grows with each insert, so near-duplicates inside the batch are caught.
                let mut recent = repo::list_item_vectors_since(conn, &since)?;
                let mut stored = 0;
                for item in pending {
                    let dup = find_duplicate_against(
                        conn,
                        &item.canonical_url,
                        &item.title_hash,
                        item.vector.as_deref(),
                        threshold,
                        &since,
                        &recent,
                    )?;
                    if let Some(Duplicate::Url | Duplicate::Title | Duplicate::Near { .. }) = dup {
                        continue;
                    }
                    repo::insert_item(conn, &item)?;
                    if let Some(v) = &item.vector {
                        recent.push((item.id.clone(), v.clone()));
                    }
                    stored += 1;
                }
                Ok(stored)
            })
            .await?;
        Ok(stored)
    }
}

enum FeedOutcome {
    NotModified,
    Stored(usize),
    Failed(String),
}

/// Title plus the first `EMBED_CHARS` characters of the body.
pub fn embed_text(title: &str, text: &str) -> String {
    let body: String = text.chars().take(EMBED_CHARS).collect();
    format!("{title}\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Env, load_config};
    use crate::core::embed::FakeEmbedder;
    use crate::core::http::client;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const LONG: &str = include_str!("../../tests/fixtures/html/long.html");
    const NOBODY: &str = include_str!("../../tests/fixtures/html/nobody.html");

    fn fixed_now() -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 9, 17, 6, 0, 0).unwrap()
    }

    fn config(budget_ms: u64) -> Config {
        let mut c = load_config(&Env::from_lookup(|_| None).unwrap()).unwrap();
        c.ingest.total_budget_ms = budget_ms;
        c.ingest.request_timeout_ms = 2000;
        c.ingest.allow_loopback = true;
        c
    }

    fn ingester(config: Config) -> Ingester {
        Ingester {
            db: Db::open_in_memory().unwrap(),
            client: client(&config.ingest).unwrap(),
            config,
            embedder: Arc::new(FakeEmbedder),
            now: fixed_now,
        }
    }

    fn feed(id: &str, url: String) -> Feed {
        Feed {
            id: id.into(),
            url,
            title: id.to_uppercase(),
            weight: 1.0,
            enabled: true,
        }
    }

    fn rss(items: &[(&str, &str)]) -> String {
        let body: String = items
            .iter()
            .map(|(title, link)| format!("<item><title>{title}</title><link>{link}</link><guid>{link}</guid><pubDate>Tue, 15 Sep 2026 08:00:00 GMT</pubDate></item>"))
            .collect();
        format!(
            "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>t</title><link>x</link><description>d</description>{body}</channel></rss>"
        )
    }

    async fn mount(server: &MockServer, p: &str, template: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(template)
            .mount(server)
            .await;
    }

    fn html_with_title(title: &str) -> String {
        LONG.replace("Long article about a small service", title)
    }

    #[tokio::test]
    async fn ingest_stores_new_items_with_vectors() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(
            &server,
            "/rss",
            ResponseTemplate::new(200).set_body_string(rss(&[
                ("One", &format!("{u}/p1?utm_source=rss")),
                ("Two", &format!("{u}/p2")),
            ])),
        )
        .await;
        mount(
            &server,
            "/p1",
            ResponseTemplate::new(200).set_body_string(html_with_title("One")),
        )
        .await;
        mount(
            &server,
            "/p2",
            ResponseTemplate::new(200).set_body_string(html_with_title("Two")),
        )
        .await;
        let ing = ingester(config(10_000));
        let report = ing.run(&[feed("a", format!("{u}/rss"))]).await.unwrap();
        assert_eq!(report.new_items, 2);
        assert_eq!(report.per_feed[0].status, FeedStatus::Ok);
        let items = ing
            .db
            .with(|c| repo::list_items_since(c, "2026-01-01T00:00:00.000Z"))
            .unwrap();
        assert_eq!(items.len(), 2);
        for i in &items {
            assert!(i.vector.is_some());
            assert_eq!(i.fetched_at, "2026-09-17T06:00:00.000Z");
            assert_eq!(i.published_at.as_deref(), Some("2026-09-15T08:00:00.000Z"));
            assert!(i.word_count > 800);
        }
        let one = items.iter().find(|i| i.title == "One").unwrap();
        assert_eq!(one.canonical_url, format!("{u}/p1"));
        assert_eq!(one.id, item_id(&format!("{u}/p1")));
        let src = ing.db.with(|c| repo::get_source(c, "a")).unwrap().unwrap();
        assert_eq!(src.last_ok_at.as_deref(), Some("2026-09-17T06:00:00.000Z"));
    }

    #[tokio::test]
    async fn ingest_skips_duplicates_by_url_and_title() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(
            &server,
            "/rss",
            ResponseTemplate::new(200).set_body_string(rss(&[
                ("One", &format!("{u}/p1")),
                ("One", &format!("{u}/p1-again")),  // same title
                ("Three", &format!("{u}/p1#frag")), // same canonical url
            ])),
        )
        .await;
        for p in ["/p1", "/p1-again"] {
            mount(
                &server,
                p,
                ResponseTemplate::new(200).set_body_string(html_with_title("x")),
            )
            .await;
        }
        let ing = ingester(config(10_000));
        let report = ing.run(&[feed("a", format!("{u}/rss"))]).await.unwrap();
        assert_eq!(report.new_items, 1, "{report:?}");
        // Second run: everything is already stored.
        let report = ing.run(&[feed("a", format!("{u}/rss"))]).await.unwrap();
        assert_eq!(report.new_items, 0);
    }

    #[tokio::test]
    async fn ingest_304_counts_as_success_with_zero_new() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(&server, "/rss", ResponseTemplate::new(304)).await;
        let ing = ingester(config(10_000));
        ing.db
            .with(|c| {
                repo::upsert_source(c, &feed("a", format!("{u}/rss")))?;
                repo::mark_source_failed(c, "a", "old")
            })
            .unwrap();
        let report = ing.run(&[feed("a", format!("{u}/rss"))]).await.unwrap();
        assert_eq!(report.per_feed[0].status, FeedStatus::NotModified);
        assert_eq!(report.new_items, 0);
        let src = ing.db.with(|c| repo::get_source(c, "a")).unwrap().unwrap();
        assert_eq!(src.failures, 0);
        assert!(src.last_error.is_none());
    }

    #[tokio::test]
    async fn ingest_failure_increments_failures_and_sets_last_error() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(&server, "/rss", ResponseTemplate::new(503)).await;
        let ing = ingester(config(10_000));
        ing.run(&[feed("a", format!("{u}/rss"))]).await.unwrap();
        let report = ing.run(&[feed("a", format!("{u}/rss"))]).await.unwrap();
        assert_eq!(report.per_feed[0].status, FeedStatus::Failed);
        assert_eq!(report.per_feed[0].error.as_deref(), Some("HTTP 503"));
        let src = ing.db.with(|c| repo::get_source(c, "a")).unwrap().unwrap();
        assert_eq!(src.failures, 2);
        assert_eq!(src.last_error.as_deref(), Some("HTTP 503"));
    }

    #[tokio::test]
    async fn ingest_success_resets_failures() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(
            &server,
            "/rss",
            ResponseTemplate::new(200).set_body_string(rss(&[])),
        )
        .await;
        let ing = ingester(config(10_000));
        ing.db
            .with(|c| {
                repo::upsert_source(c, &feed("a", format!("{u}/rss")))?;
                repo::mark_source_failed(c, "a", "old")
            })
            .unwrap();
        ing.run(&[feed("a", format!("{u}/rss"))]).await.unwrap();
        let src = ing.db.with(|c| repo::get_source(c, "a")).unwrap().unwrap();
        assert_eq!(src.failures, 0);
        assert!(src.last_error.is_none());
    }

    #[tokio::test]
    async fn ingest_persists_partial_results_on_budget_timeout() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(
            &server,
            "/fast",
            ResponseTemplate::new(200).set_body_string(rss(&[("One", &format!("{u}/p1"))])),
        )
        .await;
        mount(
            &server,
            "/p1",
            ResponseTemplate::new(200).set_body_string(html_with_title("One")),
        )
        .await;
        mount(
            &server,
            "/slow",
            ResponseTemplate::new(200)
                .set_delay(Duration::from_millis(1500))
                .set_body_string(rss(&[])),
        )
        .await;
        let ing = ingester(config(400));
        let report = ing
            .run(&[
                feed("fast", format!("{u}/fast")),
                feed("slow", format!("{u}/slow")),
            ])
            .await
            .unwrap();
        assert_eq!(report.new_items, 1);
        let slow = report.per_feed.iter().find(|r| r.id == "slow").unwrap();
        assert_eq!(slow.status, FeedStatus::Failed);
        assert_eq!(slow.error.as_deref(), Some("ingest budget exceeded"));
        let src = ing
            .db
            .with(|c| repo::get_source(c, "slow"))
            .unwrap()
            .unwrap();
        assert_eq!(src.failures, 1);
        let items = ing
            .db
            .with(|c| repo::list_items_since(c, "2026-01-01T00:00:00.000Z"))
            .unwrap();
        assert_eq!(items.len(), 1);
    }

    #[tokio::test]
    async fn ingest_skips_pages_with_no_body_and_disabled_feeds() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(
            &server,
            "/rss",
            ResponseTemplate::new(200).set_body_string(rss(&[
                ("Links", &format!("{u}/links")),
                ("Gone", &format!("{u}/gone")),
            ])),
        )
        .await;
        mount(
            &server,
            "/links",
            ResponseTemplate::new(200).set_body_string(NOBODY),
        )
        .await;
        mount(&server, "/gone", ResponseTemplate::new(404)).await;
        let ing = ingester(config(10_000));
        let mut off = feed("off", format!("{u}/rss"));
        off.enabled = false;
        let report = ing
            .run(&[feed("a", format!("{u}/rss")), off])
            .await
            .unwrap();
        assert_eq!(report.fetched, 1);
        assert_eq!(report.new_items, 0);
        assert_eq!(report.per_feed.len(), 1);
        assert!(
            ing.db
                .with(|c| repo::get_source(c, "off"))
                .unwrap()
                .is_some(),
            "disabled feeds are still mirrored"
        );
    }

    #[test]
    fn embed_text_caps_by_chars() {
        let long = "ô".repeat(5000);
        let t = embed_text("T", &long);
        assert_eq!(t.chars().count(), 2 + EMBED_CHARS);
    }

    /// An embedder that maps every text to the same unit vector: the cosine path in one stroke.
    struct SameVector;
    impl crate::core::embed::Embedder for SameVector {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, crate::core::embed::EmbedError> {
            let mut v = vec![0.0f32; crate::core::vector::DIMENSIONS];
            v[0] = 1.0;
            Ok(texts.iter().map(|_| v.clone()).collect())
        }
    }

    #[tokio::test]
    async fn ingest_skips_near_duplicate_by_cosine() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(
            &server,
            "/rss",
            ResponseTemplate::new(200).set_body_string(rss(&[
                ("First take", &format!("{u}/a")),
                ("Second take", &format!("{u}/b")),
            ])),
        )
        .await;
        mount(
            &server,
            "/a",
            ResponseTemplate::new(200).set_body_string(html_with_title("First take")),
        )
        .await;
        mount(
            &server,
            "/b",
            ResponseTemplate::new(200).set_body_string(html_with_title("Second take")),
        )
        .await;
        let config = config(10_000);
        let ing = Ingester {
            db: Db::open_in_memory().unwrap(),
            client: client(&config.ingest).unwrap(),
            config,
            embedder: Arc::new(SameVector),
            now: fixed_now,
        };
        let report = ing.run(&[feed("a", format!("{u}/rss"))]).await.unwrap();
        assert_eq!(
            report.new_items, 1,
            "the second entry is a near-duplicate of the first"
        );
        let items = ing
            .db
            .with(|c| repo::list_items_since(c, "2026-01-01T00:00:00.000Z"))
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "First take");
    }

    /// The same title seen 15 days ago is new again; seen 5 days ago it is still a duplicate.
    #[tokio::test]
    async fn title_hash_dedupe_is_scoped_to_the_window() {
        let server = MockServer::start().await;
        let u = server.uri();
        mount(
            &server,
            "/rss",
            ResponseTemplate::new(200)
                .set_body_string(rss(&[("Weekly links", &format!("{u}/this-week"))])),
        )
        .await;
        mount(
            &server,
            "/this-week",
            ResponseTemplate::new(200).set_body_string(html_with_title("Weekly links")),
        )
        .await;
        let old_item = |db: &Db, id: &str, days: i64| {
            db.with(|c| {
                repo::upsert_source(c, &feed("a", format!("{u}/rss")))?;
                repo::insert_item(
                    c,
                    &repo::NewItem {
                        id: id.into(),
                        source_id: "a".into(),
                        url: format!("{u}/{id}"),
                        canonical_url: format!("{u}/{id}"),
                        title: "Weekly links".into(),
                        author: None,
                        published_at: None,
                        fetched_at: crate::core::time::to_iso(
                            fixed_now() - chrono::Duration::days(days),
                        ),
                        text: "older links".into(),
                        word_count: 2,
                        content_hash: format!("c-{id}"),
                        title_hash: title_hash("Weekly links"),
                        vector: None,
                    },
                )
            })
            .unwrap();
        };
        let ing = ingester(config(10_000));
        old_item(&ing.db, "fifteen-days-ago", 15);
        let report = ing.run(&[feed("a", format!("{u}/rss"))]).await.unwrap();
        assert_eq!(
            report.new_items, 1,
            "a title from outside the window is new again"
        );

        let ing = ingester(config(10_000));
        old_item(&ing.db, "five-days-ago", 5);
        let report = ing.run(&[feed("a", format!("{u}/rss"))]).await.unwrap();
        assert_eq!(
            report.new_items, 0,
            "a title from inside the window is a duplicate"
        );
    }
}
