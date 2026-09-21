//! `dailybrief scan-transcript <path>`: the egress gate before `/ship`. Prints one finding per
//! line; returns 1 when anything was found, 0 when clean.

use std::io::Write;
use std::path::Path;

use crate::config::{Env, load_all};
use crate::db::Db;
use crate::db::repo;
use crate::harness::scan_transcript::{ScanOptions, scan_run_error, scan_transcript};

use super::{CommandError, db_path};

pub async fn run(env: &Env, path: &Path, out: &mut impl Write) -> Result<i32, CommandError> {
    let loaded = load_all(env)?;
    let text = std::fs::read_to_string(path)?;
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let db = Db::open(&db_path(&loaded.config))?;
    let topics = loaded.topics;
    let cap = loaded.config.caps.web_search;
    // The run directory names the run: `<data>/runs/<id>/transcript.jsonl`.
    let run_id = path
        .parent()
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned());
    let findings = db
        .call(move |conn| {
            let mut findings = scan_transcript(
                &lines,
                &ScanOptions {
                    topics: &topics,
                    forbidden: &[],
                    web_search_cap: cap,
                },
                conn,
            )?;
            if let Some(id) = run_id
                && let Some(run) = repo::get_run(conn, &id)?
                && let Some(error) = run.error
            {
                findings.extend(scan_run_error(&id, &error, &[]));
            }
            Ok(findings)
        })
        .await?;
    for f in &findings {
        writeln!(out, "{f}")?;
    }
    if findings.is_empty() {
        writeln!(out, "clean: {}", path.display())?;
        Ok(0)
    } else {
        Ok(1)
    }
}
