//! The seam between the runner and a harness adapter (`SPEC.md` §3, ADR 0001): an enum, not a
//! trait object, dispatches to the adapter; the runner sees only `HarnessRequest` and `RunOutcome`.

use std::path::PathBuf;

use super::claude_code::{ClaudeCodeAdapter, InitEvent, ResultEvent};

/// What the runner hands an adapter for one attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessRequest {
    pub run_id: String,
    /// Working directory for the child: the run directory, with no `CLAUDE.md`, `.mcp.json` or
    /// `.claude/` inside so nothing is auto-discovered.
    pub cwd: PathBuf,
    pub mcp_config_path: PathBuf,
    pub system_prompt_path: PathBuf,
    pub user_message: String,
    /// Draft-07 JSON Schema the final message must satisfy.
    pub json_schema: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailReason {
    /// `result.subtype == "error_max_turns"`.
    MaxTurns,
    /// Any other error `result`.
    Error,
    /// The process ended without a `result` line.
    NoResult,
    /// Non-zero exit (or failed to start) without a `result` line.
    Exit,
}

impl FailReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MaxTurns => "max_turns",
            Self::Error => "error",
            Self::NoResult => "no_result",
            Self::Exit => "exit",
        }
    }
}

/// The classified end of one attempt (`SPEC.md` §3 "Runner policy, precisely").
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// `result.subtype == "success"` and not `is_error`; the runner still verifies the digest.
    Success {
        result: ResultEvent,
        init: Option<InitEvent>,
    },
    Failed {
        reason: FailReason,
        message: String,
        exit_code: Option<i32>,
        result: Option<ResultEvent>,
        init: Option<InitEvent>,
    },
    /// The wall clock expired and the child was killed.
    Killed {
        message: String,
        init: Option<InitEvent>,
    },
}

impl RunOutcome {
    pub fn result(&self) -> Option<&ResultEvent> {
        match self {
            Self::Success { result, .. } => Some(result),
            Self::Failed { result, .. } => result.as_ref(),
            Self::Killed { .. } => None,
        }
    }

    pub fn init(&self) -> Option<&InitEvent> {
        match self {
            Self::Success { init, .. } | Self::Failed { init, .. } | Self::Killed { init, .. } => {
                init.as_ref()
            }
        }
    }
}

/// One harness per variant; adding Codex (M4) is one variant and one match arm.
#[derive(Debug, Clone)]
pub enum HarnessKind {
    ClaudeCode(ClaudeCodeAdapter),
}

impl HarnessKind {
    /// The `runs.harness` value.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ClaudeCode(_) => "claude-code",
        }
    }
}
