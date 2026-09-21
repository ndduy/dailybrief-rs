//! CLI verbs (`spec/r0.md` §4). Each verb is a function over the loaded config; `main.rs` maps
//! `CommandError` into its exit code.

pub mod fetch;
pub mod mcp;
pub mod migrate;
pub mod prune;
pub mod reembed;
pub mod run;
pub mod scan_transcript;
pub mod serve;

use std::path::PathBuf;

use crate::config::{Config, ConfigError};
use crate::db::DbError;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("cannot write output: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Ingest(#[from] crate::core::ingest::IngestError),
    #[error(transparent)]
    Http(#[from] crate::core::http::HttpError),
    #[error("cannot encode output: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Usage(String),
    #[error("mcp: {0}")]
    Mcp(String),
    #[error(transparent)]
    Runner(#[from] crate::harness::runner::RunnerError),
    #[error(transparent)]
    Retention(#[from] crate::core::retention::RetentionError),
    #[error(transparent)]
    Env(#[from] crate::harness::claude_code::EnvError),
    #[error(transparent)]
    Bind(#[from] crate::web::app::BindError),
    #[error("cannot bind {addr}: {source}")]
    Listen {
        addr: String,
        #[source]
        source: std::io::Error,
    },
}

/// The SQLite file inside the data directory.
pub fn db_path(config: &Config) -> PathBuf {
    config.paths.data_dir.join("brief.db")
}
