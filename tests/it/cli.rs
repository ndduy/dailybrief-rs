//! The binary's argument validation, through the real executable.

use std::process::Command;

#[test]
fn prune_days_below_seven_is_refused_by_the_cli() {
    let out = Command::new(env!("CARGO_BIN_EXE_dailybrief"))
        .args(["prune", "--days", "3", "--dry-run"])
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .output()
        .expect("the binary runs");
    assert_eq!(out.status.code(), Some(2), "clap's usage error exit code");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--days"), "{err}");
    assert!(err.contains('7'), "{err}");
}
