//! `dailybrief scan-transcript <path>` or `--run <id>`: the egress gate before `/ship`. Prints
//! one finding per line; returns 1 when anything was found, 0 when clean.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::{Config, Env, load_all};
use crate::db::Db;
use crate::db::repo;
use crate::harness::scan_transcript::{ScanOptions, scan_run_error, scan_transcript};

use super::{CommandError, db_path};

/// The transcript to scan: an explicit path, or `<data>/runs/<id>/transcript.jsonl` for
/// `--run <id>` (exactly one of the two).
pub fn resolve_path(
    config: &Config,
    path: Option<&Path>,
    run: Option<&str>,
) -> Result<PathBuf, CommandError> {
    match (path, run) {
        (Some(p), None) => Ok(p.to_path_buf()),
        (None, Some(id)) => {
            if !crate::core::retention::id_is_a_plain_name(id) {
                return Err(CommandError::Usage(format!(
                    "scan-transcript: '{id}' is not a run id"
                )));
            }
            Ok(
                crate::harness::runner::run_dir(&config.paths.data_dir, id)
                    .join("transcript.jsonl"),
            )
        }
        _ => Err(CommandError::Usage(
            "scan-transcript: give a transcript path or --run <id>, not both".into(),
        )),
    }
}

pub async fn run(
    env: &Env,
    path: Option<&Path>,
    run_id: Option<&str>,
    out: &mut impl Write,
) -> Result<i32, CommandError> {
    let loaded = load_all(env)?;
    let path = resolve_path(&loaded.config, path, run_id)?;
    let path = path.as_path();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_transcript_accepts_run_id() {
        let config = load_all(&Env::from_lookup(|_| None).unwrap())
            .unwrap()
            .config;
        let by_id = resolve_path(&config, None, Some("2026-09-22-7494d2ef")).unwrap();
        assert_eq!(
            by_id,
            config
                .paths
                .data_dir
                .join("runs/2026-09-22-7494d2ef/transcript.jsonl")
        );
        let explicit = resolve_path(&config, Some(Path::new("/tmp/t.jsonl")), None).unwrap();
        assert_eq!(explicit, PathBuf::from("/tmp/t.jsonl"));
        assert!(resolve_path(&config, None, Some("../etc")).is_err());
        assert!(resolve_path(&config, None, None).is_err());
        assert!(resolve_path(&config, Some(Path::new("x")), Some("y")).is_err());
    }
}
