//! `dailybrief reembed`: re-embed every item and topic once with the current model.

use std::io::Write;

use crate::config::{Env, load_all};
use crate::core::embed::embedder_for;
use crate::core::reembed::reembed_all;
use crate::db::Db;

use super::{CommandError, db_path};

pub async fn run(env: &Env, out: &mut impl Write) -> Result<(), CommandError> {
    let loaded = load_all(env)?;
    let db = Db::open(&db_path(&loaded.config))?;
    let report = reembed_all(&db, embedder_for(&loaded.config), &loaded.topics).await?;
    writeln!(out, "{}", serde_json::to_string_pretty(&report)?)?;
    Ok(())
}
