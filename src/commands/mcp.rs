//! `dailybrief mcp`: the stdio MCP server the harness spawns via the per-run `mcp.json`.
//! stdout is the protocol stream; every log line goes to stderr (set up in `main`).

use rmcp::ServiceExt;

use crate::config::{Env, load_all};
use crate::core::embed::embedder_for;
use crate::core::http::client;
use crate::db::Db;
use crate::db::repo::Role;
use crate::mcp::server::DailyBriefServer;

use super::{CommandError, db_path};

/// `DAILYBRIEF_RUN_ROLE` picks the tool set; unset means the Editor (every R0 run).
pub fn role_from_env(env: &Env) -> Result<Role, CommandError> {
    match env.run_role.as_deref() {
        None => Ok(Role::Editor),
        Some(s) => Role::parse(s).ok_or_else(|| {
            CommandError::Usage(format!(
                "DAILYBRIEF_RUN_ROLE must be 'editor' or 'curator', got '{s}'"
            ))
        }),
    }
}

pub async fn run(env: &Env) -> Result<(), CommandError> {
    let run_id = env
        .run_id
        .clone()
        .ok_or_else(|| CommandError::Usage("DAILYBRIEF_RUN_ID is required for `mcp`".into()))?;
    let role = role_from_env(env)?;
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
    )
    .with_role(role);
    tracing::info!(run_id = %server.run_id, role = role.as_str(), "mcp server starting on stdio");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(role: Option<&str>) -> Env {
        Env::from_lookup(|name| match name {
            "DAILYBRIEF_RUN_ROLE" => role.map(str::to_string),
            _ => None,
        })
        .unwrap()
    }

    #[test]
    fn mcp_verb_defaults_to_editor_without_the_env() {
        assert_eq!(role_from_env(&env_with(None)).unwrap(), Role::Editor);
        assert_eq!(
            role_from_env(&env_with(Some("editor"))).unwrap(),
            Role::Editor
        );
        assert_eq!(
            role_from_env(&env_with(Some("curator"))).unwrap(),
            Role::Curator
        );
        let err = role_from_env(&env_with(Some("boss"))).unwrap_err();
        assert!(err.to_string().contains("DAILYBRIEF_RUN_ROLE"), "{err}");
    }
}
