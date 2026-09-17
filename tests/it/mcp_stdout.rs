//! `dailybrief mcp` must write nothing but JSON-RPC to stdout, even with logging at trace.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

#[tokio::test]
async fn mcp_stdout_is_json_rpc_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_dailybrief"))
        .arg("mcp")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("DAILYBRIEF_CONFIG", root.join("config/config.toml"))
        .env("DAILYBRIEF_DATA_DIR", dir.path())
        .env("DAILYBRIEF_RUN_ID", "2026-09-17-stdout-test")
        .env("RUST_LOG", "trace")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn dailybrief mcp");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;
    let initialized = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    for line in [init, initialized, list] {
        stdin.write_all(line.as_bytes()).await.unwrap();
        stdin.write_all(b"\n").await.unwrap();
    }
    stdin.flush().await.unwrap();

    let mut lines = BufReader::new(stdout).lines();
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while seen.len() < 2 {
        let next = tokio::time::timeout_at(deadline, lines.next_line()).await;
        match next {
            Ok(Ok(Some(line))) => seen.push(line),
            Ok(Ok(None)) => break,
            Ok(Err(e)) => panic!("stdout read failed: {e}"),
            Err(_) => panic!("timed out waiting for two JSON-RPC responses; got {seen:?}"),
        }
    }
    drop(stdin);
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;

    assert_eq!(seen.len(), 2, "{seen:?}");
    for line in &seen {
        let v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("non-JSON on stdout: {line:?} ({e})"));
        assert_eq!(v["jsonrpc"], "2.0", "{line}");
    }
    let tools = &serde_json::from_str::<serde_json::Value>(&seen[1]).unwrap()["result"]["tools"];
    assert!(
        tools
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "get_briefing"),
        "{tools}"
    );
}
