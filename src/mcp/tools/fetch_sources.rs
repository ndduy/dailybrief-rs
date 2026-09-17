//! `fetch_sources()` → `{ fetched, newItems, perFeed: [{ id, source, status, newItems, error? }] }`.
//! Idempotent within the process: the second call returns the first report without refetching.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use rmcp::model::CallToolResult;

use crate::core::ingest::Ingester;
use crate::mcp::error::{ToolError, ok};
use crate::mcp::server::DailyBriefServer;

pub async fn run(s: &DailyBriefServer) -> Result<CallToolResult, ToolError> {
    if s.fetched.swap(true, Ordering::SeqCst) {
        let stored = s
            .first_fetch_report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(report) = stored {
            return Ok(ok(report));
        }
    }
    let ingester = Ingester {
        db: s.db.clone(),
        config: s.config.clone(),
        client: s.client.clone(),
        embedder: Arc::clone(&s.embedder),
        now: s.now,
    };
    let report = ingester
        .run(&s.feeds)
        .await
        .map_err(|e| ToolError::Internal(e.to_string()))?;
    let value = serde_json::to_value(&report).map_err(|e| ToolError::Internal(e.to_string()))?;
    *s.first_fetch_report
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(value.clone());
    Ok(ok(value))
}

#[cfg(test)]
mod tests {
    use crate::config::Feed;
    use crate::db::Db;
    use crate::mcp::server::testkit::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn fetch_sources_is_idempotent_within_run() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/rss"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>t</title><link>x</link><description>d</description></channel></rss>",
            ))
            .expect(1)
            .mount(&server)
            .await;
        let feeds = vec![Feed {
            id: "only".into(),
            url: format!("{}/rss", server.uri()),
            title: "Only".into(),
            weight: 1.0,
            enabled: true,
        }];
        let h = Harness::start(server_with_feeds(Db::open_in_memory().unwrap(), feeds)).await;
        let first = structured(&h.call("fetch_sources", json!({})).await);
        assert_eq!(first["fetched"], 1);
        assert_eq!(first["perFeed"][0]["id"], "only");
        assert_eq!(first["perFeed"][0]["status"], "ok");
        let second = structured(&h.call("fetch_sources", json!({})).await);
        assert_eq!(first, second, "second call returns the first report");
        server.verify().await; // exactly one request reached the feed
    }
}
