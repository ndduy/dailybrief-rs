//! Configuration: `config/config.toml`, `feeds.toml`, `topics.toml` and the `DAILYBRIEF_*` env.
//! Files are parsed into `*File` structs (exact TOML shape, unknown keys rejected) and validated
//! into the resolved types the rest of the crate consumes.

mod load;
mod schema;

pub use load::{Env, Loaded, load_all, load_config, load_feeds, load_topics};
pub use schema::{
    Caps, ClaudeCodeSettings, Config, ConfigFile, Embeddings, Feed, FeedsFile, HarnessName,
    HarnessSettings, Ingest, Paths, Schedule, Service, Topic, TopicOrigin, TopicsFile, is_valid_id,
};

use std::path::PathBuf;

/// Every way loading or validating configuration can fail. Messages name the file and the field.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("{file}: {message}")]
    Invalid { file: &'static str, message: String },
    #[error("environment variable {name}: {message}")]
    Env { name: &'static str, message: String },
}
