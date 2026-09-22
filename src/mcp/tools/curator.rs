//! The five Curator tools (`spec/m3.md` curator-tools): `get_feedback` and `get_profile` wrap
//! `core::curator_input`; `find_feeds` and `validate_feed` wrap `core::feeds_discovery` (and
//! remember every URL that validated, for `add_source`); `propose_change` follows in Task 8.

use rmcp::model::CallToolResult;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::core::curator_input;
use crate::core::feedback::Evidence;
use crate::core::feeds_discovery;
use crate::mcp::error::{ToolError, ok, rejected};
use crate::mcp::server::DailyBriefServer;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UrlInput {
    /// An absolute http(s) URL.
    pub url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProposeChangeInput {
    /// One of: topic_weight, add_topic, disable_source, add_source, promote_explore_topic.
    pub kind: String,
    /// The payload for that kind (see the prompt for the fields and bounds).
    pub payload: Value,
    /// Why: a one-paragraph summary, the rating ids, the read count, the feed issue ids, notes.
    pub evidence: Evidence,
}

pub async fn get_feedback(s: &DailyBriefServer) -> Result<CallToolResult, ToolError> {
    let now = (s.now)();
    let fb =
        s.db.call(move |conn| curator_input::feedback(conn, now, curator_input::WINDOW_DAYS))
            .await?;
    Ok(ok(
        serde_json::to_value(fb).map_err(|e| ToolError::Internal(e.to_string()))?
    ))
}

pub async fn get_profile(s: &DailyBriefServer) -> Result<CallToolResult, ToolError> {
    let p = s.db.call(|conn| curator_input::profile(conn)).await?;
    Ok(ok(
        serde_json::to_value(p).map_err(|e| ToolError::Internal(e.to_string()))?
    ))
}

/// `{ feeds: [{ url, title?, type }] }`; a page that cannot be fetched is a typed error.
pub async fn find_feeds(s: &DailyBriefServer, i: UrlInput) -> Result<CallToolResult, ToolError> {
    let feeds =
        feeds_discovery::find_feeds(&s.client, &i.url, (s.now)(), s.config.ingest.allow_loopback)
            .await
            .map_err(|e| ToolError::Feed(e.to_string()))?;
    Ok(ok(serde_json::json!({ "feeds": feeds })))
}

/// `{ ok: true, title, type, items, itemsPerDay, lastItemAt }`, or `{ ok: false, error }` as an
/// error result the prompt can quote. A validated URL is remembered for `propose_change`.
pub async fn validate_feed(s: &DailyBriefServer, i: UrlInput) -> Result<CallToolResult, ToolError> {
    match feeds_discovery::validate_feed(&s.client, &i.url, (s.now)()).await {
        Ok(v) => {
            s.validated_feeds.lock().await.insert(i.url.clone());
            Ok(ok(
                serde_json::to_value(v).map_err(|e| ToolError::Internal(e.to_string()))?
            ))
        }
        Err(e) => Ok(rejected(
            serde_json::json!({ "ok": false, "error": e.to_string() }),
        )),
    }
}

pub async fn propose_change(
    _s: &DailyBriefServer,
    _i: ProposeChangeInput,
) -> Result<CallToolResult, ToolError> {
    Err(ToolError::NotImplemented("propose_change"))
}

#[cfg(test)]
mod tests {
    use crate::db::Db;
    use crate::db::repo::Role;
    use crate::mcp::server::testkit::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const RSS: &str = "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Feed T</title><link>https://x.example/</link><description>d</description><item><title>P</title><link>https://x.example/p</link><pubDate>Wed, 16 Sep 2026 08:00:00 GMT</pubDate></item></channel></rss>";

    #[tokio::test]
    async fn find_feeds_and_validate_feed_through_mcp_and_remember_validated_urls() {
        let server = MockServer::start().await;
        let u = server.uri();
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><head><link rel=\"alternate\" type=\"application/rss+xml\" title=\"T\" href=\"{u}/rss\"></head></html>"
            )))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/rss"))
            .respond_with(ResponseTemplate::new(200).set_body_string(RSS))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let srv = server_with(Db::open_in_memory().unwrap()).with_role(Role::Curator);
        let validated = std::sync::Arc::clone(&srv.validated_feeds);
        let h = Harness::start(srv).await;
        let r = h
            .call("find_feeds", json!({ "url": format!("{u}/") }))
            .await;
        assert_eq!(r.is_error, Some(false), "{r:?}");
        assert_eq!(
            structured(&r)["feeds"],
            json!([{ "url": format!("{u}/rss"), "title": "T", "type": "rss" }])
        );
        let r = h
            .call("validate_feed", json!({ "url": format!("{u}/rss") }))
            .await;
        assert_eq!(r.is_error, Some(false), "{r:?}");
        let v = structured(&r);
        assert_eq!(v["ok"], true);
        assert_eq!(v["title"], "Feed T");
        assert_eq!(v["type"], "rss");
        assert_eq!(v["items"], 1);
        assert_eq!(v["lastItemAt"], "2026-09-16T08:00:00.000Z");
        assert!(v.get("itemsPerDay").is_some());
        assert!(validated.lock().await.contains(&format!("{u}/rss")));
        let r = h
            .call("validate_feed", json!({ "url": format!("{u}/nope") }))
            .await;
        assert_eq!(r.is_error, Some(true));
        assert_eq!(structured(&r), json!({ "ok": false, "error": "HTTP 404" }));
        assert!(!validated.lock().await.contains(&format!("{u}/nope")));
        let r = h
            .call("find_feeds", json!({ "url": "http://10.0.0.1/" }))
            .await;
        assert_eq!(r.is_error, Some(true));
        assert!(
            structured(&r)["error"]
                .as_str()
                .unwrap()
                .contains("private address")
        );
    }
}
