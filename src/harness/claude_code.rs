//! The `claude-code` adapter (`SPEC.md` §3): the exact argv, the scrubbed environment that keeps
//! the run on the subscription, and the `stream-json` events the runner relies on.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

use super::types::{FailReason, HarnessRequest, RunOutcome};
use crate::config::ClaudeCodeSettings;

/// How much of stderr is kept for the failure message.
const STDERR_TAIL: usize = 4096;

pub const MCP_SERVER_TOOL_PATTERN: &str = "mcp__dailybrief__*";
pub const USER_MESSAGE: &str = "Build today's digest. Start with get_briefing.";

/// Variables the child may see, plus anything prefixed `DAILYBRIEF_`.
pub const ENV_ALLOWLIST: [&str; 8] = [
    "PATH",
    "HOME",
    "LANG",
    "TZ",
    "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "DISABLE_AUTOUPDATER",
    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
];
/// Variables that outrank the subscription: their presence aborts the run before it spends anything.
pub const FORBIDDEN_ENV: [&str; 2] = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"];
pub const OAUTH_TOKEN_VAR: &str = "CLAUDE_CODE_OAUTH_TOKEN";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EnvError {
    #[error("{0} is set in the service environment; it would outrank the subscription")]
    Forbidden(&'static str),
    #[error("CLAUDE_CODE_OAUTH_TOKEN is missing; mint one with `claude setup-token`")]
    MissingToken,
}

/// The `claude -p` invocation, flag for flag (`SPEC.md` §3).
pub fn build_argv(settings: &ClaudeCodeSettings, req: &HarnessRequest) -> Vec<String> {
    vec![
        "-p".into(),
        req.user_message.clone(),
        "--system-prompt-file".into(),
        req.system_prompt_path.to_string_lossy().into_owned(),
        "--restricted".into(),
        "--tools".into(),
        "WebSearch".into(),
        "--strict-mcp-config".into(),
        "--mcp-config".into(),
        req.mcp_config_path.to_string_lossy().into_owned(),
        "--allowedTools".into(),
        MCP_SERVER_TOOL_PATTERN.into(),
        "WebSearch".into(),
        "--permission-prompts".into(),
        "none".into(),
        "--max-turns".into(),
        settings.max_turns.to_string(),
        "--model".into(),
        settings.model.clone(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--json-schema".into(),
        req.json_schema.to_string(),
        "--no-session-persistence".into(),
    ]
}

/// Builds the child environment from the allowlist; refuses anything that would change billing.
pub fn build_env(
    parent: &HashMap<String, String>,
    passthrough: &[&str],
) -> Result<HashMap<String, String>, EnvError> {
    for key in FORBIDDEN_ENV {
        if parent.contains_key(key) {
            return Err(EnvError::Forbidden(key));
        }
    }
    if parent
        .get(OAUTH_TOKEN_VAR)
        .is_none_or(|v| v.trim().is_empty())
    {
        return Err(EnvError::MissingToken);
    }
    Ok(parent
        .iter()
        .filter(|(k, _)| {
            ENV_ALLOWLIST.contains(&k.as_str())
                || k.starts_with("DAILYBRIEF_")
                || passthrough.contains(&k.as_str())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect())
}

// ---------- stream-json events (only the fields the runner relies on; extras pass) ----------

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    #[serde(default)]
    pub status: Option<String>,
}

/// `{"type":"system","subtype":"init", ...}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct InitEvent {
    pub subtype: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerStatus>,
}

/// `{"type":"result","subtype":"success"|"error_max_turns"|..., ...}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ResultEvent {
    pub subtype: String,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub duration_api_ms: Option<u64>,
    #[serde(default)]
    pub num_turns: Option<u64>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub usage: Option<Value>,
    #[serde(default)]
    pub structured_output: Option<Value>,
}

impl ResultEvent {
    /// The `result` field as text when it is a string (error messages), else the subtype.
    pub fn message(&self) -> String {
        match &self.result {
            Some(Value::String(s)) => s.clone(),
            _ => self.subtype.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Init(InitEvent),
    Result(ResultEvent),
    /// Any other well-formed event (`assistant`, `user`, `rate_limit_event`, …), kept raw.
    Other {
        kind: String,
        raw: Value,
    },
    Unparseable(String),
}

/// Classifies one stdout line. A `system` event that is not `init` is `Other`.
pub fn parse_stream_line(raw: &str) -> StreamEvent {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return StreamEvent::Unparseable(raw.to_string());
    };
    let Some(kind) = value
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return StreamEvent::Unparseable(raw.to_string());
    };
    match kind.as_str() {
        "system" if value.get("subtype").and_then(Value::as_str) == Some("init") => {
            match serde_json::from_value::<InitEvent>(value.clone()) {
                Ok(init) => StreamEvent::Init(init),
                Err(_) => StreamEvent::Other { kind, raw: value },
            }
        }
        "result" => match serde_json::from_value::<ResultEvent>(value.clone()) {
            Ok(result) => StreamEvent::Result(result),
            Err(_) => StreamEvent::Other { kind, raw: value },
        },
        _ => StreamEvent::Other { kind, raw: value },
    }
}

/// The `run_events.type` value for a raw line: its `type` field, or `unparseable`.
pub fn event_type(raw: &str) -> String {
    match parse_stream_line(raw) {
        StreamEvent::Init(_) => "system".to_string(),
        StreamEvent::Result(_) => "result".to_string(),
        StreamEvent::Other { kind, .. } => kind,
        StreamEvent::Unparseable(_) => "unparseable".to_string(),
    }
}

/// How one `claude -p` process is spawned (Task 19 adds `run`).
#[derive(Debug, Clone)]
pub struct ClaudeCodeAdapter {
    pub settings: ClaudeCodeSettings,
    /// The executable; `claude` on `PATH` by default, a fake in tests.
    pub binary: PathBuf,
    pub env: HashMap<String, String>,
    pub wall_clock: Duration,
    pub kill_grace: Duration,
}

impl ClaudeCodeAdapter {
    /// Scrubs `parent` once, up front, so a forbidden variable fails before any spawn.
    pub fn new(
        settings: ClaudeCodeSettings,
        parent: &HashMap<String, String>,
    ) -> Result<Self, EnvError> {
        let env = build_env(parent, &[])?;
        let wall_clock = Duration::from_secs(u64::from(settings.wall_clock_minutes) * 60);
        Ok(Self {
            settings,
            binary: PathBuf::from("claude"),
            env,
            wall_clock,
            kill_grace: Duration::from_secs(10),
        })
    }

    /// Spawns `claude -p`, streams stdout to `on_line` while draining stderr, enforces the wall
    /// clock (SIGTERM, then SIGKILL after `kill_grace`), and classifies the end of the process.
    pub async fn run(
        &self,
        req: &HarnessRequest,
        mut on_line: impl FnMut(&str, u64),
    ) -> RunOutcome {
        let failed = |reason, message: String, exit_code| RunOutcome::Failed {
            reason,
            message,
            exit_code,
            result: None,
            init: None,
        };
        let mut child = match Command::new(&self.binary)
            .args(build_argv(&self.settings, req))
            .current_dir(&req.cwd)
            .env_clear()
            .envs(&self.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                return failed(
                    FailReason::Exit,
                    format!("could not start {}: {e}", self.binary.display()),
                    None,
                );
            }
        };
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            return failed(
                FailReason::Exit,
                "child has no stdout/stderr pipes".into(),
                None,
            );
        };
        // stderr is drained concurrently so a chatty child can never block on a full pipe.
        let mut stderr_task = tokio::spawn(async move {
            let mut tail: Vec<u8> = Vec::new();
            let mut reader = BufReader::new(stderr);
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        tail.extend_from_slice(&buf[..n]);
                        if tail.len() > STDERR_TAIL {
                            let cut = tail.len() - STDERR_TAIL;
                            tail.drain(..cut);
                        }
                    }
                }
            }
            String::from_utf8_lossy(&tail).trim().to_string()
        });

        let mut init: Option<InitEvent> = None;
        let mut result: Option<ResultEvent> = None;
        let mut seq: u64 = 0;
        let read_stdout = async {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(raw)) = lines.next_line().await {
                if raw.trim().is_empty() {
                    continue;
                }
                seq += 1;
                on_line(&raw, seq);
                match parse_stream_line(&raw) {
                    StreamEvent::Init(i) => init = Some(i),
                    StreamEvent::Result(r) => result = Some(r),
                    _ => {}
                }
            }
        };
        // One deadline covers the read loop *and* the wait after EOF: a child that closes its
        // pipes and then hangs would otherwise hold the run lock (and the scheduler) forever.
        let deadline = tokio::time::Instant::now() + self.wall_clock;
        let timed_out = tokio::time::timeout_at(deadline, read_stdout)
            .await
            .is_err();
        let status = if timed_out {
            None
        } else {
            match tokio::time::timeout_at(deadline, child.wait()).await {
                Ok(status) => Some(status.ok()),
                Err(_) => None,
            }
        };
        let Some(status) = status else {
            terminate(&mut child, self.kill_grace).await;
            drain(&mut stderr_task, self.kill_grace).await;
            return RunOutcome::Killed {
                message: format!("wall clock of {} s exceeded", self.wall_clock.as_secs()),
                init,
            };
        };
        let exit_code = status.and_then(|s| s.code());
        let stderr_tail = drain(&mut stderr_task, self.kill_grace).await;
        match result {
            Some(r) if r.subtype == "success" && !r.is_error => {
                RunOutcome::Success { result: r, init }
            }
            Some(r) => {
                let reason = if r.subtype == "error_max_turns" {
                    FailReason::MaxTurns
                } else {
                    FailReason::Error
                };
                RunOutcome::Failed {
                    reason,
                    message: r.message(),
                    exit_code,
                    result: Some(r),
                    init,
                }
            }
            None => match exit_code {
                Some(0) => RunOutcome::Failed {
                    reason: FailReason::NoResult,
                    message: "process ended without a result message".into(),
                    exit_code,
                    result: None,
                    init,
                },
                code => RunOutcome::Failed {
                    reason: FailReason::Exit,
                    message: match code {
                        Some(c) => format!("exit code {c}: {stderr_tail}"),
                        None => format!("killed by signal: {stderr_tail}"),
                    },
                    exit_code: code,
                    result: None,
                    init,
                },
            },
        }
    }
}

/// The stderr tail, or an empty string if the drain task does not end within `grace` (a
/// grandchild that inherited the pipe keeps it open; the task is aborted so nothing leaks).
async fn drain(task: &mut tokio::task::JoinHandle<String>, grace: Duration) -> String {
    match tokio::time::timeout(grace, &mut *task).await {
        Ok(Ok(tail)) => tail,
        _ => {
            task.abort();
            String::new()
        }
    }
}

/// SIGTERM first (the child can flush), SIGKILL after `grace` (`SPEC.md` §3); `nix` keeps this
/// free of `unsafe`.
async fn terminate(child: &mut tokio::process::Child, grace: Duration) {
    if let Some(pid) = child.id() {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGTERM,
        );
    }
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Env, load_config};

    fn settings() -> ClaudeCodeSettings {
        load_config(&Env::from_lookup(|_| None).unwrap())
            .unwrap()
            .harness
            .claude_code
    }

    fn request() -> HarnessRequest {
        HarnessRequest {
            run_id: "2026-09-17-abcd1234".into(),
            cwd: PathBuf::from("/data/runs/2026-09-17-abcd1234"),
            mcp_config_path: PathBuf::from("/data/runs/2026-09-17-abcd1234/mcp.json"),
            system_prompt_path: PathBuf::from("/app/prompts/editor.md"),
            user_message: USER_MESSAGE.into(),
            json_schema: serde_json::json!({ "type": "object", "properties": { "date": { "type": "string" } } }),
        }
    }

    fn parent(extra: &[(&str, &str)]) -> HashMap<String, String> {
        let mut m: HashMap<String, String> = [
            ("PATH", "/usr/bin"),
            ("HOME", "/home/app"),
            ("LANG", "C.UTF-8"),
            ("TZ", "Asia/Ho_Chi_Minh"),
            ("CLAUDE_CONFIG_DIR", "/home/app/.claude"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat-example"),
            ("DISABLE_AUTOUPDATER", "1"),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
            ("DAILYBRIEF_DATA_DIR", "/data"),
            ("SHELL", "/bin/zsh"),
            ("AWS_SECRET_ACCESS_KEY", "nope"),
            ("EDITOR", "vim"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        for (k, v) in extra {
            m.insert((*k).to_string(), (*v).to_string());
        }
        m
    }

    #[test]
    fn argv_snapshot() {
        let argv = build_argv(&settings(), &request());
        insta::assert_debug_snapshot!(argv);
    }

    #[test]
    fn argv_never_contains_bare_and_carries_every_spec_flag() {
        let argv = build_argv(&settings(), &request());
        assert!(!argv.iter().any(|a| a == "--bare"));
        for flag in [
            "-p",
            "--system-prompt-file",
            "--restricted",
            "--tools",
            "--strict-mcp-config",
            "--mcp-config",
            "--allowedTools",
            "--permission-prompts",
            "--max-turns",
            "--model",
            "--output-format",
            "--verbose",
            "--json-schema",
            "--no-session-persistence",
        ] {
            assert!(argv.iter().any(|a| a == flag), "missing {flag}");
        }
        let max_turns_at = argv.iter().position(|a| a == "--max-turns").unwrap();
        assert_eq!(argv[max_turns_at + 1], "120");
        let schema_at = argv.iter().position(|a| a == "--json-schema").unwrap();
        assert!(
            argv[schema_at + 1].starts_with('{'),
            "schema is inline JSON"
        );
        assert_eq!(argv[1], USER_MESSAGE);
    }

    #[test]
    fn env_refuses_anthropic_api_key_and_auth_token() {
        assert_eq!(
            build_env(&parent(&[("ANTHROPIC_API_KEY", "sk-ant-x")]), &[]).unwrap_err(),
            EnvError::Forbidden("ANTHROPIC_API_KEY")
        );
        assert_eq!(
            build_env(&parent(&[("ANTHROPIC_AUTH_TOKEN", "t")]), &[]).unwrap_err(),
            EnvError::Forbidden("ANTHROPIC_AUTH_TOKEN")
        );
    }

    #[test]
    fn env_requires_oauth_token() {
        let mut p = parent(&[]);
        p.remove("CLAUDE_CODE_OAUTH_TOKEN");
        assert_eq!(build_env(&p, &[]).unwrap_err(), EnvError::MissingToken);
        p.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), "   ".into());
        assert_eq!(build_env(&p, &[]).unwrap_err(), EnvError::MissingToken);
    }

    #[test]
    fn env_passes_only_allowlist_and_dailybrief_prefix_and_drops_everything_else() {
        let env = build_env(&parent(&[("DAILYBRIEF_RUN_ID", "r")]), &[]).unwrap();
        let mut keys: Vec<&str> = env.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDE_CONFIG_DIR",
                "DAILYBRIEF_DATA_DIR",
                "DAILYBRIEF_RUN_ID",
                "DISABLE_AUTOUPDATER",
                "HOME",
                "LANG",
                "PATH",
                "TZ"
            ]
        );
        assert!(!env.contains_key("AWS_SECRET_ACCESS_KEY"));
        let with_extra = build_env(&parent(&[]), &["EDITOR"]).unwrap();
        assert_eq!(with_extra.get("EDITOR").map(String::as_str), Some("vim"));
    }

    const SUCCESS: &str = include_str!("../../tests/fixtures/transcripts/success.jsonl");
    const MAX_TURNS: &str = include_str!("../../tests/fixtures/transcripts/max-turns.jsonl");

    #[test]
    fn parses_init_event() {
        let first = SUCCESS.lines().next().unwrap();
        match parse_stream_line(first) {
            StreamEvent::Init(init) => {
                assert_eq!(init.subtype, "init");
                assert_eq!(init.model.as_deref(), Some("claude-opus-5"));
                assert_eq!(init.mcp_servers[0].name, "dailybrief");
                assert!(
                    init.tools
                        .iter()
                        .any(|t| t == "mcp__dailybrief__get_briefing")
                );
                assert!(init.tools.iter().any(|t| t == "WebSearch"));
            }
            other => panic!("expected init, got {other:?}"),
        }
        assert_eq!(event_type(first), "system");
    }

    #[test]
    fn parses_result_success_with_structured_output() {
        let last = SUCCESS.lines().last().unwrap();
        match parse_stream_line(last) {
            StreamEvent::Result(r) => {
                assert_eq!(r.subtype, "success");
                assert!(!r.is_error);
                assert_eq!(r.num_turns, Some(87));
                assert_eq!(r.structured_output.as_ref().unwrap()["forYou"], 24);
                assert_eq!(r.session_id.as_deref(), Some("sess-success"));
                assert!(r.total_cost_usd.is_some());
            }
            other => panic!("expected result, got {other:?}"),
        }
    }

    #[test]
    fn parses_result_error_max_turns() {
        let last = MAX_TURNS.lines().last().unwrap();
        match parse_stream_line(last) {
            StreamEvent::Result(r) => {
                assert_eq!(r.subtype, "error_max_turns");
                assert!(r.is_error);
                assert_eq!(r.message(), "Reached maximum number of turns (120)");
                assert!(r.structured_output.is_none());
            }
            other => panic!("expected result, got {other:?}"),
        }
    }

    #[test]
    fn passes_through_unknown_types_and_keeps_unparseable_raw() {
        match parse_stream_line(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#,
        ) {
            StreamEvent::Other { kind, raw } => {
                assert_eq!(kind, "rate_limit_event");
                assert_eq!(raw["rate_limit_info"]["status"], "allowed");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            event_type(r#"{"type":"assistant","message":{"content":[]}}"#),
            "assistant"
        );
        assert_eq!(
            parse_stream_line("not json at all"),
            StreamEvent::Unparseable("not json at all".into())
        );
        assert_eq!(event_type(r#"{"no":"type"}"#), "unparseable");
        assert_eq!(
            event_type(r#"{"type":"system","subtype":"hook"}"#),
            "system"
        );
    }

    #[test]
    fn no_libc_signal_path() {
        // Only the production half of this file: the test module names the forbidden crate.
        let src = include_str!("claude_code.rs");
        let production = src.split("#[cfg(test)]").next().unwrap_or_default();
        let forbidden = ["libc", "::"].concat();
        assert!(
            !production.contains(&forbidden),
            "signals go through nix, never {forbidden}"
        );
    }

    #[test]
    fn adapter_new_scrubs_env_up_front() {
        let ok = ClaudeCodeAdapter::new(settings(), &parent(&[])).unwrap();
        assert_eq!(ok.wall_clock, Duration::from_secs(15 * 60));
        assert!(!ok.env.contains_key("SHELL"));
        let err =
            ClaudeCodeAdapter::new(settings(), &parent(&[("ANTHROPIC_API_KEY", "x")])).unwrap_err();
        assert_eq!(err, EnvError::Forbidden("ANTHROPIC_API_KEY"));
    }
}
