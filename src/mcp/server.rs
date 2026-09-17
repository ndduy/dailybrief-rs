//! The MCP server the harness spawns (`dailybrief mcp`). Tools are thin wrappers over `core`
//! functions; this file owns the router, the per-run state, and the guard that turns every
//! failure (including a panic) into an `isError` result.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8};

use chrono::{DateTime, Utc};
use futures_util::FutureExt;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde_json::Value;

use crate::config::{Config, Feed};
use crate::core::embed::Embedder;
use crate::db::Db;

use super::error::ToolError;
use super::tools;

pub const SERVER_NAME: &str = "dailybrief";

/// Everything the nine tools need for one run. The MCP process lives for one run, so the two
/// atomics are the only in-process state (`fetch_sources` idempotency, publish rejection count).
#[derive(Clone)]
pub struct DailyBriefServer {
    pub db: Db,
    pub config: Config,
    pub feeds: Vec<Feed>,
    pub run_id: String,
    pub embedder: Arc<dyn Embedder>,
    pub client: reqwest::Client,
    pub now: fn() -> DateTime<Utc>,
    pub fetched: Arc<AtomicBool>,
    pub publish_rejections: Arc<AtomicU8>,
    pub first_fetch_report: Arc<std::sync::Mutex<Option<Value>>>,
}

impl DailyBriefServer {
    pub fn new(
        db: Db,
        config: Config,
        feeds: Vec<Feed>,
        run_id: String,
        embedder: Arc<dyn Embedder>,
        client: reqwest::Client,
        now: fn() -> DateTime<Utc>,
    ) -> Self {
        Self {
            db,
            config,
            feeds,
            run_id,
            embedder,
            client,
            now,
            fetched: Arc::new(AtomicBool::new(false)),
            publish_rejections: Arc::new(AtomicU8::new(0)),
            first_fetch_report: Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

/// Runs a tool body; a `ToolError` or a panic becomes an `isError` result.
pub async fn guarded<F>(fut: F) -> CallToolResult
where
    F: std::future::Future<Output = Result<CallToolResult, ToolError>>,
{
    match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => e.into_result(),
        Err(_) => ToolError::Internal("tool panicked".into()).into_result(),
    }
}

#[tool_router]
impl DailyBriefServer {
    /// Fetch today's dynamic context in one call: the date, your topic profile (names and weights),
    /// the item ids shown in the last 14 days, feed health, your notes, and every cap. Call this first.
    #[tool]
    async fn get_briefing(&self) -> CallToolResult {
        guarded(tools::get_briefing::run(self)).await
    }

    /// Pull every enabled feed now (conditional GET), extract new articles, dedupe and embed them.
    /// Returns counts per feed. Calling it again in the same run returns the first report.
    #[tool]
    async fn fetch_sources(&self) -> CallToolResult {
        guarded(tools::fetch_sources::run(self)).await
    }

    /// Record that a feed is dead, paywalled, junk or a duplicate so the Curator can act on it.
    /// Use the feed id from get_briefing's feedHealth.
    #[tool]
    async fn report_feed_issue(
        &self,
        Parameters(input): Parameters<tools::report_feed_issue::ReportFeedIssueInput>,
    ) -> CallToolResult {
        guarded(tools::report_feed_issue::run(self, input)).await
    }

    /// Read or replace your notes: one text of at most 2000 characters that survives across runs.
    /// Use it for what you learned about sources and topics.
    #[tool]
    async fn editor_notes(
        &self,
        Parameters(input): Parameters<tools::editor_notes::EditorNotesInput>,
    ) -> CallToolResult {
        guarded(tools::editor_notes::run(self, input)).await
    }
}

#[tool_handler]
impl ServerHandler for DailyBriefServer {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        let mut info = rmcp::model::ServerConfig::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        );
        info.server_info = rmcp::model::Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION"));
        info.instructions =
            Some("Tools for building the daily digest. Start with get_briefing.".into());
        info
    }
}

#[cfg(test)]
pub(crate) mod testkit {
    //! An in-process client over a duplex pipe, for every MCP test.
    use super::*;
    use crate::config::{Env, load_all};
    use crate::core::embed::FakeEmbedder;
    use crate::core::http::client;
    use rmcp::ServiceExt;
    use rmcp::model::{CallToolRequestParams, JsonObject};
    use rmcp::service::RunningService;
    use rmcp::{RoleClient, RoleServer};

    pub const RUN: &str = "2026-09-17-mcp-test";

    pub fn fixed_now() -> DateTime<Utc> {
        chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 9, 17, 6, 0, 0).unwrap()
    }

    pub fn server_with(db: Db) -> DailyBriefServer {
        let loaded = load_all(&Env::from_lookup(|_| None).unwrap()).unwrap();
        server_with_feeds(db, loaded.feeds)
    }

    /// A server whose `fetch_sources` targets `feeds` (tests point them at wiremock).
    pub fn server_with_feeds(db: Db, feeds: Vec<Feed>) -> DailyBriefServer {
        let loaded = load_all(&Env::from_lookup(|_| None).unwrap()).unwrap();
        DailyBriefServer::new(
            db,
            loaded.config.clone(),
            feeds,
            RUN.into(),
            Arc::new(FakeEmbedder),
            client(&loaded.config.ingest).unwrap(),
            fixed_now,
        )
    }

    pub struct Harness {
        pub client: RunningService<RoleClient, ()>,
        _server: RunningService<RoleServer, DailyBriefServer>,
    }

    impl Harness {
        pub async fn start(server: DailyBriefServer) -> Self {
            let (client_side, server_side) = tokio::io::duplex(1 << 16);
            let (sr, sw) = tokio::io::split(server_side);
            let (cr, cw) = tokio::io::split(client_side);
            // The server side waits for the client's initialize request, so both handshakes
            // must run concurrently.
            let (server, client) = tokio::join!(server.serve((sr, sw)), ().serve((cr, cw)));
            Self {
                client: client.expect("client connects"),
                _server: server.expect("server starts"),
            }
        }

        pub async fn tool_names(&self) -> Vec<String> {
            let mut names: Vec<String> = self
                .client
                .list_all_tools()
                .await
                .unwrap()
                .into_iter()
                .map(|t| t.name.to_string())
                .collect();
            names.sort();
            names
        }

        pub async fn call(&self, name: &str, args: Value) -> CallToolResult {
            let arguments: Option<JsonObject> = match args {
                Value::Object(m) => Some(m),
                Value::Null => None,
                other => panic!("tool arguments must be an object, got {other}"),
            };
            let mut params = CallToolRequestParams::new(name.to_string());
            params.arguments = arguments;
            self.client.call_tool(params).await.unwrap()
        }
    }

    /// The structured content of a result, or a panic with the text content for context.
    pub fn structured(r: &CallToolResult) -> Value {
        r.structured_content
            .clone()
            .unwrap_or_else(|| panic!("no structured content: {:?}", r.content))
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn tool_error_becomes_is_error_result_not_protocol_error() {
        let r =
            guarded(async { Err::<CallToolResult, _>(ToolError::UnknownFeed("f".into())) }).await;
        assert_eq!(r.is_error, Some(true));
        assert!(
            structured(&r)["error"]
                .as_str()
                .unwrap()
                .starts_with("No feed with id 'f'")
        );
    }

    #[tokio::test]
    async fn internal_panic_becomes_one_sentence() {
        let r = guarded(async {
            if fixed_now().timestamp() > 0 {
                panic!("boom");
            }
            Ok::<_, ToolError>(super::super::error::ok(json!({})))
        })
        .await;
        assert_eq!(r.is_error, Some(true));
        assert_eq!(
            structured(&r)["error"],
            "The tool failed internally; try once more, then move on."
        );
        let text = r.content[0].as_text().expect("text content");
        assert!(text.text.contains("failed internally"));
    }

    #[tokio::test]
    async fn tools_list_has_get_briefing_with_camel_case_fields() {
        let h = Harness::start(server_with(Db::open_in_memory().unwrap())).await;
        assert_eq!(
            h.tool_names().await,
            vec![
                "editor_notes",
                "fetch_sources",
                "get_briefing",
                "report_feed_issue"
            ]
        );
        let tools = h.client.list_all_tools().await.unwrap();
        let t = tools.iter().find(|t| t.name == "get_briefing").unwrap();
        assert!(
            t.description
                .as_deref()
                .unwrap()
                .contains("Call this first")
        );
        let r = h.call("get_briefing", json!({})).await;
        let s = structured(&r);
        for key in [
            "date",
            "timezone",
            "profile",
            "shownIds",
            "feedHealth",
            "notes",
            "caps",
        ] {
            assert!(s.get(key).is_some(), "missing {key}: {s}");
        }
        for key in [
            "reads",
            "forYou",
            "beyondRadar",
            "perSource",
            "perTopic",
            "webSearch",
        ] {
            assert!(s["caps"].get(key).is_some(), "missing caps.{key}: {s}");
        }
    }
}
