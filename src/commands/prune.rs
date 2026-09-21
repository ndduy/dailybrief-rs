//! `dailybrief prune [--days N] [--dry-run]`: the retention policy by hand (ADR 0013). Prints
//! the report as JSON; `serve` runs the same function on the `[retention].cron` schedule.

use std::io::Write;

use crate::config::{Env, load_config};
use crate::core::retention::prune;
use crate::db::Db;

use super::{CommandError, db_path};

pub async fn run(
    env: &Env,
    days: Option<u32>,
    dry_run: bool,
    out: &mut impl Write,
) -> Result<i32, CommandError> {
    let config = load_config(env)?;
    let db = Db::open(&db_path(&config))?;
    let days = days.unwrap_or(config.retention.days);
    let report = prune(
        &db,
        &config.paths.data_dir,
        chrono::Utc::now(),
        days,
        dry_run,
    )
    .await?;
    writeln!(out, "{}", serde_json::to_string_pretty(&report)?)?;
    Ok(0)
}
