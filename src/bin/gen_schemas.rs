//! `cargo run --bin gen-schemas [-- --check]`: regenerate `schemas/digest.json` from
//! `editor::DigestOutput` and `schemas/curator.json` from `editor::CuratorOutput`, or with
//! `--check` verify each committed file matches as parsed JSON.
use std::path::PathBuf;
use std::process::ExitCode;

use dailybrief::editor::{curator_output, digest_output};

fn main() -> ExitCode {
    let check = std::env::args().any(|a| a == "--check");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let files = [
        (
            "schemas/digest.json",
            "editor::DigestOutput",
            digest_output::schema(),
        ),
        (
            "schemas/curator.json",
            "editor::CuratorOutput",
            curator_output::schema(),
        ),
    ];
    let mut worst = ExitCode::SUCCESS;
    for (rel, source, generated) in files {
        let path = root.join(rel);
        let code = if check {
            check_one(&path, source, &generated)
        } else {
            write_one(&path, &generated)
        };
        if code != ExitCode::SUCCESS {
            worst = code;
        }
    }
    worst
}

fn check_one(path: &std::path::Path, source: &str, generated: &serde_json::Value) -> ExitCode {
    let committed = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("gen-schemas: cannot read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };
    let committed: serde_json::Value = match serde_json::from_str(&committed) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("gen-schemas: {} is not valid JSON: {e}", path.display());
            return ExitCode::from(1);
        }
    };
    if &committed == generated {
        println!("gen-schemas: {} is up to date", path.display());
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "gen-schemas: {} differs from {source}; run `cargo run --bin gen-schemas`",
            path.display()
        );
        ExitCode::from(1)
    }
}

fn write_one(path: &std::path::Path, generated: &serde_json::Value) -> ExitCode {
    let rendered = match serde_json::to_string_pretty(generated) {
        Ok(s) => format!("{s}\n"),
        Err(e) => {
            eprintln!("gen-schemas: cannot render schema: {e}");
            return ExitCode::from(2);
        }
    };
    match std::fs::write(path, rendered) {
        Ok(()) => {
            println!("gen-schemas: wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("gen-schemas: cannot write {}: {e}", path.display());
            ExitCode::from(2)
        }
    }
}
