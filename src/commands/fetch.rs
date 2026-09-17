//! `dailybrief fetch`: ingest every enabled feed once, no agent; prints the report as JSON.

use std::io::Write;

use crate::config::{Env, load_all};
use crate::core::embed::embedder_for;
use crate::core::http::client;
use crate::core::ingest::{IngestReport, Ingester};
use crate::db::Db;

use super::{CommandError, db_path};

pub async fn run(env: &Env, out: &mut impl Write) -> Result<IngestReport, CommandError> {
    let loaded = load_all(env)?;
    let db = Db::open(&db_path(&loaded.config))?;
    let ingester = Ingester {
        db,
        client: client(&loaded.config.ingest)?,
        embedder: embedder_for(&loaded.config),
        config: loaded.config,
        now: chrono::Utc::now,
    };
    let report = ingester.run(&loaded.feeds).await?;
    writeln!(out, "{}", serde_json::to_string_pretty(&report)?)?;
    Ok(report)
}
