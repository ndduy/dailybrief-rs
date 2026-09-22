//! The MCP server the harness spawns (`dailybrief mcp`). Tools are thin wrappers over `core`
//! functions; this file owns the routers, the per-run state, and the guard that turns every
//! failure (including a panic) into an `isError` result.
//!
//! One server type, two tool sets: the Editor's nine tools and the Curator's five live in two
//! `#[tool_router]` impls, and the server holds the router for its [`Role`] (`with_role`). An
//! Editor run therefore cannot even see `propose_change`.

use std::sync::Arc;
use std::sync::atomic::AtomicU8;

use chrono::{DateTime, Utc};
use futures_util::FutureExt;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde_json::Value;

use crate::config::{Config, Feed};
use crate::core::embed::Embedder;
use crate::core::http::Http;
use crate::db::Db;
use crate::db::repo::Role;

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
    pub client: Http,
    pub now: fn() -> DateTime<Utc>,
    pub publish_rejections: Arc<AtomicU8>,
    /// The first `fetch_sources` report; the lock is held across the ingest, so a concurrent
    /// second call waits and gets the same report instead of fetching again.
    pub fetch_report: Arc<tokio::sync::Mutex<Option<Value>>>,
    pub role: Role,
    /// The tool set for `role`; `list_tools` and `call_tool` go through it and nothing else.
    tool_router: Arc<ToolRouter<Self>>,
}

impl DailyBriefServer {
    pub fn new(
        db: Db,
        config: Config,
        feeds: Vec<Feed>,
        run_id: String,
        embedder: Arc<dyn Embedder>,
        client: Http,
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
            publish_rejections: Arc::new(AtomicU8::new(0)),
            fetch_report: Arc::new(tokio::sync::Mutex::new(None)),
            role: Role::Editor,
            tool_router: Arc::new(Self::editor_router()),
        }
    }

    /// Swaps in the tool set for `role` (the constructor gives the Editor's).
    pub fn with_role(mut self, role: Role) -> Self {
        self.role = role;
        self.tool_router = Arc::new(match role {
            Role::Editor => Self::editor_router(),
            Role::Curator => Self::curator_router(),
        });
        self
    }

    /// The tool names this server lists, sorted.
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        names
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

#[tool_router(router = editor_router)]
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

    /// Rank the last 7 days of unseen items. exploit = closest to your profile; cold_topic = nearest to
    /// topics with no recent positive signal; popular_unmatched = low profile match, ordered by source
    /// weight then recency. Returns id, score, title, source, published, snippet.
    #[tool]
    async fn list_candidates(
        &self,
        Parameters(input): Parameters<tools::list_candidates::ListCandidatesInput>,
    ) -> CallToolResult {
        guarded(tools::list_candidates::run(self, input)).await
    }

    /// Embedding search over the last 7 days for a plain-language query (at most 30 results).
    /// Use it only when an explore slot lacks a candidate.
    #[tool]
    async fn search_items(
        &self,
        Parameters(input): Parameters<tools::search_items::SearchItemsInput>,
    ) -> CallToolResult {
        guarded(tools::search_items::run(self, input)).await
    }

    /// Read an item's extracted text (first 5000 characters). You must read an item before you can
    /// select it; at most 45 distinct items per run, re-reads are free.
    #[tool]
    async fn read_item(
        &self,
        Parameters(input): Parameters<tools::read_item::ReadItemInput>,
    ) -> CallToolResult {
        guarded(tools::read_item::run(self, input)).await
    }

    /// Stage an item you have read into for_you or beyond_radar with a summary (≤ 80 words), why it
    /// matters (≤ 25 words), its topic, and for beyond_radar a reason. section "none" un-stages it.
    /// Caps: 24 for_you, 6 beyond_radar, 4 per source, 8 per topic; nothing shown in the last 14 days.
    #[tool]
    async fn select(
        &self,
        Parameters(input): Parameters<tools::select::SelectToolInput>,
    ) -> CallToolResult {
        guarded(tools::select::run(self, input)).await
    }

    /// Validate the staged set end to end and publish it as today's digest. Returns the digest id,
    /// or the list of violations to fix. After three rejections the run is over.
    #[tool]
    async fn publish_digest(&self) -> CallToolResult {
        guarded(tools::publish_digest::run(self)).await
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

#[tool_router(router = curator_router)]
impl DailyBriefServer {
    /// The week's evidence in one call: ratings with reasons, reads, the explore hit rate and
    /// the feed issues, over the window since the last Curator run. Call this first.
    #[tool]
    async fn get_feedback(&self) -> CallToolResult {
        guarded(tools::curator::get_feedback(self)).await
    }

    /// The current profile: topics (id, name, weight, origin, saturation, last positive) and
    /// sources (id, title, url, weight, enabled, failures, last ok).
    #[tool]
    async fn get_profile(&self) -> CallToolResult {
        guarded(tools::curator::get_profile(self)).await
    }

    /// Discover the RSS/Atom feeds a web page advertises. Returns url, title and type per feed.
    #[tool]
    async fn find_feeds(
        &self,
        Parameters(input): Parameters<tools::curator::UrlInput>,
    ) -> CallToolResult {
        guarded(tools::curator::find_feeds(self, input)).await
    }

    /// Fetch and parse one feed URL: ok, title, items per day, last item date, or the error.
    /// Validate before you propose add_source.
    #[tool]
    async fn validate_feed(
        &self,
        Parameters(input): Parameters<tools::curator::UrlInput>,
    ) -> CallToolResult {
        guarded(tools::curator::validate_feed(self, input)).await
    }

    /// Propose one change to the profile with its evidence. Nothing changes until the reader
    /// approves it on the web; the answer is the proposal id or a typed rejection to fix.
    #[tool]
    async fn propose_change(
        &self,
        Parameters(input): Parameters<tools::curator::ProposeChangeInput>,
    ) -> CallToolResult {
        guarded(tools::curator::propose_change(self, input)).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for DailyBriefServer {
    fn get_info(&self) -> rmcp::model::ServerConfig {
        let mut info = rmcp::model::ServerConfig::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        );
        info.server_info = rmcp::model::Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION"));
        info.instructions = Some(
            match self.role {
                Role::Editor => "Tools for building the daily digest. Start with get_briefing.",
                Role::Curator => "Tools for curating the reading profile. Start with get_feedback.",
            }
            .into(),
        );
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
        // The tool tests fetch from a wiremock on 127.0.0.1.
        let mut loaded = loaded;
        loaded.config.ingest.allow_loopback = true;
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
    async fn an_editor_server_has_no_propose_change() {
        let h = Harness::start(server_with(Db::open_in_memory().unwrap())).await;
        let mut params = rmcp::model::CallToolRequestParams::new("propose_change".to_string());
        params.arguments = None;
        let err = h.client.call_tool(params).await.unwrap_err();
        assert!(err.to_string().contains("tool not found"), "{err}");
        assert!(!h.tool_names().await.contains(&"propose_change".to_string()));
    }

    #[tokio::test]
    async fn a_curator_server_lists_the_five_tools_and_none_of_the_editors() {
        let server = server_with(Db::open_in_memory().unwrap()).with_role(Role::Curator);
        assert_eq!(server.role, Role::Curator);
        assert_eq!(server.tool_names().len(), 5);
        let h = Harness::start(server).await;
        assert_eq!(
            h.tool_names().await,
            vec![
                "find_feeds",
                "get_feedback",
                "get_profile",
                "propose_change",
                "validate_feed"
            ]
        );
        let info = h.client.peer_info().unwrap();
        assert!(
            info.instructions
                .as_deref()
                .unwrap()
                .contains("get_feedback")
        );
        let r = h
            .call("find_feeds", json!({"url": "https://x.example/"}))
            .await;
        assert_eq!(r.is_error, Some(true), "a stub until Task 7");
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
                "list_candidates",
                "publish_digest",
                "read_item",
                "report_feed_issue",
                "search_items",
                "select"
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
