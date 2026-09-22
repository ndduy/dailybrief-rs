//! The `PreToolUse` hook (`spec/m3.md` §4, ADR 0019): a second layer in front of the Curator's
//! tools. `decide` is pure: tool name + input + the run's counter → allow or refuse with one
//! sentence. `dailybrief hook pre-tool-use` wraps it with Claude Code's stdin JSON and exit
//! codes (0 allow, 2 refuse with the reason on stderr); the runner renders the per-run
//! `settings.json` that points Claude Code at it (`--settings`, honoured under `--restricted`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::core::feedback::{Evidence, ProposalChange};
use crate::mcp::tools::curator::KINDS;

/// The Curator's web-search cap (`spec/m3.md` §3); the ninth call is refused.
pub const CURATOR_WEB_SEARCH_CAP: u32 = 8;
pub const PROPOSE_TOOL: &str = "mcp__dailybrief__propose_change";
pub const WEB_SEARCH_TOOL: &str = "WebSearch";
pub const STATE_FILE: &str = "hook-state.json";
pub const SETTINGS_FILE: &str = "settings.json";
pub const EVENT: &str = "pre-tool-use";

/// What the hook remembers across calls in one run, in `<run dir>/hook-state.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HookState {
    pub web_searches: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Refuse(String),
}

/// The policy. Anything but the two tools it knows is allowed untouched.
pub fn decide(tool: &str, input: &Value, state: &HookState) -> Decision {
    match tool {
        WEB_SEARCH_TOOL => {
            if state.web_searches >= CURATOR_WEB_SEARCH_CAP {
                Decision::Refuse(format!(
                    "WebSearch refused: the cap is {CURATOR_WEB_SEARCH_CAP} per run and it is used up."
                ))
            } else {
                Decision::Allow
            }
        }
        PROPOSE_TOOL => match check_proposal(input) {
            Ok(()) => Decision::Allow,
            Err(reason) => Decision::Refuse(format!("propose_change refused: {reason}")),
        },
        _ => Decision::Allow,
    }
}

/// The same bounds `propose_change` enforces, applied before the call reaches the server.
fn check_proposal(input: &Value) -> Result<(), String> {
    let kind = input
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "kind is missing.".to_string())?;
    if !KINDS.contains(&kind) {
        return Err(format!(
            "kind must be one of {}; got '{kind}'.",
            KINDS.join(", ")
        ));
    }
    let payload = input.get("payload").cloned().unwrap_or(Value::Null);
    let change: ProposalChange =
        serde_json::from_value(json!({ "kind": kind, "payload": payload }))
            .map_err(|e| format!("payload for {kind} does not parse: {e}."))?;
    change.validate().map_err(|e| e.to_string())?;
    let evidence: Evidence =
        serde_json::from_value(input.get("evidence").cloned().unwrap_or(Value::Null))
            .map_err(|e| format!("evidence does not parse: {e}."))?;
    evidence.validate().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn state_path(run_dir: &Path) -> PathBuf {
    run_dir.join(STATE_FILE)
}

/// The counter file, or the zero state when it is absent or unreadable.
pub fn load_state(run_dir: &Path) -> HookState {
    std::fs::read_to_string(state_path(run_dir))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_state(run_dir: &Path, state: &HookState) -> std::io::Result<()> {
    let text = serde_json::to_string(state).map_err(std::io::Error::other)?;
    std::fs::write(state_path(run_dir), text)
}

/// What Claude Code writes on the hook's stdin (the fields this policy reads).
#[derive(Debug, Deserialize)]
struct HookInput {
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    tool_input: Value,
    #[serde(default)]
    cwd: Option<PathBuf>,
}

/// One hook invocation: `(exit code, stderr text)`. Unparsable input allows (the hook must
/// never block a call it cannot read); an allowed `WebSearch` is counted in the run dir.
pub fn handle(stdin: &str, fallback_cwd: &Path) -> (i32, String) {
    let Ok(input) = serde_json::from_str::<HookInput>(stdin) else {
        return (0, String::new());
    };
    let run_dir = input.cwd.unwrap_or_else(|| fallback_cwd.to_path_buf());
    let mut state = load_state(&run_dir);
    match decide(&input.tool_name, &input.tool_input, &state) {
        Decision::Allow => {
            if input.tool_name == WEB_SEARCH_TOOL {
                state.web_searches += 1;
                if let Err(e) = save_state(&run_dir, &state) {
                    // A lost counter would let the 9th search through; the post-hoc scan
                    // (`scan-transcript`) is the second layer, so allow and note it.
                    return (0, format!("hook: could not save {STATE_FILE}: {e}"));
                }
            }
            (0, String::new())
        }
        Decision::Refuse(reason) => (2, reason),
    }
}

/// The per-run `settings.json`: one `PreToolUse` entry whose matcher names the two tools and
/// whose command is this binary's `hook` verb.
pub fn render_settings(exe: &Path) -> String {
    let command = format!("{} hook {EVENT}", shell_quote(&exe.to_string_lossy()));
    let value = json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": format!("{PROPOSE_TOOL}|{WEB_SEARCH_TOOL}"),
                "hooks": [{ "type": "command", "command": command, "timeout": 10 }]
            }]
        }
    });
    let mut text = serde_json::to_string_pretty(&value).unwrap_or_default();
    text.push('\n');
    text
}

/// Single-quoted for `sh`; a quote inside becomes `'\''`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(kind: &str) -> Value {
        json!({
            "kind": kind,
            "payload": { "topicId": "rust", "weight": 2.0 },
            "evidence": { "summary": "three off_topic ratings", "ratingIds": [1, 2, 3] }
        })
    }

    #[test]
    fn hook_allows_valid_proposals_and_refuses_unknown_kinds() {
        let s = HookState::default();
        assert_eq!(
            decide(PROPOSE_TOOL, &proposal("topic_weight"), &s),
            Decision::Allow
        );
        let Decision::Refuse(r) = decide(PROPOSE_TOOL, &proposal("rename_topic"), &s) else {
            panic!("unknown kind allowed");
        };
        assert!(
            r.starts_with("propose_change refused: kind must be one of"),
            "{r}"
        );
        let Decision::Refuse(r) = decide(
            PROPOSE_TOOL,
            &json!({ "kind": "topic_weight", "payload": { "topicId": "rust", "weight": 9 }, "evidence": { "summary": "s" } }),
            &s,
        ) else {
            panic!("out-of-bounds weight allowed");
        };
        assert!(r.contains("weight must be within"), "{r}");
        let Decision::Refuse(r) = decide(
            PROPOSE_TOOL,
            &json!({ "kind": "disable_source", "payload": {}, "evidence": { "summary": "s" } }),
            &s,
        ) else {
            panic!("missing field allowed");
        };
        assert!(r.contains("does not parse"), "{r}");
        let Decision::Refuse(r) = decide(
            PROPOSE_TOOL,
            &json!({ "kind": "topic_weight", "payload": { "topicId": "rust", "weight": 1 } }),
            &s,
        ) else {
            panic!("missing evidence allowed");
        };
        assert!(r.contains("evidence"), "{r}");
        assert!(matches!(
            decide(PROPOSE_TOOL, &json!({}), &s),
            Decision::Refuse(_)
        ));
    }

    #[test]
    fn hook_refuses_the_ninth_web_search_and_counts_per_run() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let call = |cwd: &Path| {
            let stdin = json!({ "hook_event_name": "PreToolUse", "tool_name": "WebSearch", "tool_input": { "query": "Rust" }, "cwd": cwd }).to_string();
            handle(&stdin, Path::new("/nonexistent"))
        };
        for i in 1..=8 {
            assert_eq!(call(dir.path()), (0, String::new()), "search {i}");
        }
        assert_eq!(load_state(dir.path()).web_searches, 8);
        let (code, reason) = call(dir.path());
        assert_eq!(code, 2);
        assert!(reason.contains("cap is 8"), "{reason}");
        assert_eq!(
            load_state(dir.path()).web_searches,
            8,
            "a refused call is not counted"
        );
        // Another run directory has its own counter.
        assert_eq!(call(other.path()), (0, String::new()));
        assert_eq!(load_state(other.path()).web_searches, 1);
    }

    #[test]
    fn hook_leaves_other_tools_alone() {
        let s = HookState { web_searches: 99 };
        for tool in [
            "mcp__dailybrief__get_feedback",
            "mcp__dailybrief__validate_feed",
            "Read",
            "",
        ] {
            assert_eq!(
                decide(tool, &json!({ "anything": 1 }), &s),
                Decision::Allow,
                "{tool}"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let (code, err) = handle("not json at all", dir.path());
        assert_eq!(
            (code, err.as_str()),
            (0, ""),
            "unreadable input never blocks"
        );
        let (code, _) = handle(
            &json!({ "tool_name": "Read", "tool_input": {} }).to_string(),
            dir.path(),
        );
        assert_eq!(code, 0);
        assert!(
            !state_path(dir.path()).exists(),
            "other tools leave no counter"
        );
    }

    #[test]
    fn settings_json_names_both_tools_and_this_binary() {
        let text = render_settings(Path::new("/usr/local/bin/dailybrief"));
        let v: Value = serde_json::from_str(&text).unwrap();
        let entry = &v["hooks"]["PreToolUse"][0];
        assert_eq!(
            entry["matcher"],
            "mcp__dailybrief__propose_change|WebSearch"
        );
        assert_eq!(entry["hooks"][0]["type"], "command");
        assert_eq!(
            entry["hooks"][0]["command"],
            "'/usr/local/bin/dailybrief' hook pre-tool-use"
        );
        let odd: Value =
            serde_json::from_str(&render_settings(Path::new("/tmp/it's here/dailybrief"))).unwrap();
        assert_eq!(
            odd["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "'/tmp/it'\\''s here/dailybrief' hook pre-tool-use"
        );
    }
}
