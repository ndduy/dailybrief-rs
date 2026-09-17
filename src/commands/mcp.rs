//! `dailybrief mcp`: the stdio MCP server the harness spawns via the per-run `mcp.json`.
//! stdout is the protocol stream; every log line goes to stderr (set up in `main`).

use rmcp::ServiceExt;

use crate::config::{Env, load_all};
use crate::core::embed::embedder_for;
use crate::core::http::client;
use crate::db::Db;
use crate::mcp::server::DailyBriefServer;

use super::{CommandError, db_path};

pub async fn run(env: &Env) -> Result<(), CommandError> {
    let run_id = env
        .run_id
        .clone()
        .ok_or_else(|| CommandError::Usage("DAILYBRIEF_RUN_ID is required for `mcp`".into()))?;
    let loaded = load_all(env)?;
    let db = Db::open(&db_path(&loaded.config))?;
    let server = DailyBriefServer::new(
        db,
        loaded.config.clone(),
        loaded.feeds,
        run_id,
        embedder_for(&loaded.config),
        client(&loaded.config.ingest)?,
        chrono::Utc::now,
    );
    tracing::info!(run_id = %server.run_id, "mcp server starting on stdio");
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| CommandError::Mcp(e.to_string()))?;
    running
        .waiting()
        .await
        .map_err(|e| CommandError::Mcp(e.to_string()))?;
    Ok(())
}
