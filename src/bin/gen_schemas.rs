//! `cargo run --bin gen-schemas [-- --check]`: regenerate `schemas/digest.json` from
//! `editor::DigestOutput`, or with `--check` verify the committed file matches as parsed JSON.
use std::path::PathBuf;
use std::process::ExitCode;

use dailybrief::editor::digest_output::schema;

fn main() -> ExitCode {
    let check = std::env::args().any(|a| a == "--check");
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas/digest.json");
    let generated = schema();
    let rendered = match serde_json::to_string_pretty(&generated) {
        Ok(s) => format!("{s}\n"),
        Err(e) => {
            eprintln!("gen-schemas: cannot render schema: {e}");
            return ExitCode::from(2);
        }
    };
    if check {
        let committed = match std::fs::read_to_string(&path) {
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
        if committed == generated {
            println!("gen-schemas: {} is up to date", path.display());
            ExitCode::SUCCESS
        } else {
            eprintln!(
                "gen-schemas: {} differs from editor::DigestOutput; run `cargo run --bin gen-schemas`",
                path.display()
            );
            ExitCode::from(1)
        }
    } else {
        match std::fs::write(&path, rendered) {
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
}
