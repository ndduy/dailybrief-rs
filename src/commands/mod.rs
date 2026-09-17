//! CLI verbs (`spec/r0.md` §4). Each verb is a function over the loaded config; `main.rs` maps
//! `CommandError` into its exit code.

pub mod migrate;

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
}

/// The SQLite file inside the data directory.
pub fn db_path(config: &Config) -> PathBuf {
    config.paths.data_dir.join("brief.db")
}
