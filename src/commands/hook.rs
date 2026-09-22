//! `dailybrief hook <event>`: the body Claude Code runs for the per-run `PreToolUse` hook.
//! Reads the call JSON on stdin, exits 0 to allow or 2 to refuse with the reason on stderr.
//! Reads no config and opens no database: it must answer in milliseconds, in any cwd.

use std::io::{Read, Write};
use std::path::Path;

use crate::harness::hook;

use super::CommandError;

pub fn run(
    event: &str,
    stdin: &mut impl Read,
    cwd: &Path,
    stderr: &mut impl Write,
) -> Result<i32, CommandError> {
    if event != hook::EVENT {
        return Err(CommandError::Usage(format!(
            "hook: unknown event '{event}' (only {})",
            hook::EVENT
        )));
    }
    let mut input = String::new();
    stdin.read_to_string(&mut input)?;
    let (code, message) = hook::handle(&input, cwd);
    if !message.is_empty() {
        writeln!(stderr, "{message}")?;
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hook_verb_exits_2_with_the_reason_on_stderr_and_0_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let bad = json!({ "tool_name": "mcp__dailybrief__propose_change", "tool_input": { "kind": "x" }, "cwd": dir.path() }).to_string();
        let mut err = Vec::new();
        let code = run("pre-tool-use", &mut bad.as_bytes(), dir.path(), &mut err).unwrap();
        assert_eq!(code, 2);
        assert!(
            String::from_utf8(err)
                .unwrap()
                .starts_with("propose_change refused:")
        );
        let ok =
            json!({ "tool_name": "mcp__dailybrief__get_profile", "tool_input": {} }).to_string();
        let mut err = Vec::new();
        assert_eq!(
            run("pre-tool-use", &mut ok.as_bytes(), dir.path(), &mut err).unwrap(),
            0
        );
        assert!(err.is_empty());
        let e = run(
            "post-tool-use",
            &mut "{}".as_bytes(),
            dir.path(),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(e.to_string().contains("unknown event"));
    }
}
