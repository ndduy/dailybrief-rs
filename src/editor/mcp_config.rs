//! Renders the per-run `mcp.json` from the committed template (`SPEC.md` §3): `${MCP_COMMAND}` is
//! this binary, `${MCP_ARGS}` the JSON array of args, plus the run id, its role, config path and
//! data dir.

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum McpConfigError {
    #[error("mcp.json template is not valid JSON after substitution: {0}")]
    Json(#[from] serde_json::Error),
    #[error("mcp.json template still contains '{0}' after substitution")]
    Unsubstituted(String),
}

pub struct McpConfigVars<'a> {
    pub command: &'a Path,
    pub args: &'a [String],
    pub run_id: &'a str,
    /// `editor` or `curator` (`DAILYBRIEF_RUN_ROLE` for the `mcp` verb).
    pub role: &'a str,
    pub config_path: &'a Path,
    pub data_dir: &'a Path,
}

/// Substitutes the placeholders, drops `_comment`, and pretty-prints valid JSON.
pub fn render(template: &str, vars: &McpConfigVars<'_>) -> Result<String, McpConfigError> {
    let args_json = serde_json::to_string(vars.args)?;
    let filled = template
        .replace("\"${MCP_ARGS}\"", &args_json)
        .replace(
            "${MCP_COMMAND}",
            &json_escape(&vars.command.to_string_lossy()),
        )
        .replace("${RUN_ID}", &json_escape(vars.run_id))
        .replace("${RUN_ROLE}", &json_escape(vars.role))
        .replace(
            "${CONFIG_PATH}",
            &json_escape(&vars.config_path.to_string_lossy()),
        )
        .replace(
            "${DATA_DIR}",
            &json_escape(&vars.data_dir.to_string_lossy()),
        );
    let mut value: serde_json::Value = serde_json::from_str(&filled)?;
    if let Some(obj) = value.as_object_mut() {
        obj.remove("_comment");
    }
    let rendered = serde_json::to_string_pretty(&value)?;
    if let Some(start) = rendered.find("${") {
        let end = rendered[start..]
            .find('}')
            .map_or(rendered.len(), |e| start + e + 1);
        return Err(McpConfigError::Unsubstituted(
            rendered[start..end].to_string(),
        ));
    }
    Ok(format!("{rendered}\n"))
}

/// Escapes a value for placement inside a JSON string literal (the template quotes it).
fn json_escape(raw: &str) -> String {
    let quoted = serde_json::to_string(raw).unwrap_or_default();
    quoted.trim_matches('"').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const TEMPLATE: &str = include_str!("../../config/mcp.json");

    fn vars<'a>(
        cmd: &'a Path,
        args: &'a [String],
        cfg: &'a Path,
        data: &'a Path,
    ) -> McpConfigVars<'a> {
        McpConfigVars {
            command: cmd,
            args,
            run_id: "2026-09-17-abcd1234",
            role: "editor",
            config_path: cfg,
            data_dir: data,
        }
    }

    #[test]
    fn render_mcp_config_substitutes_all_placeholders_and_drops_comment() {
        let cmd = PathBuf::from("/usr/local/bin/dailybrief");
        let args = vec!["mcp".to_string()];
        let cfg = PathBuf::from("/app/config/config.toml");
        let data = PathBuf::from("/data");
        let out = render(TEMPLATE, &vars(&cmd, &args, &cfg, &data)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v.get("_comment").is_none());
        let server = &v["mcpServers"]["dailybrief"];
        assert_eq!(server["command"], "/usr/local/bin/dailybrief");
        assert_eq!(server["args"], serde_json::json!(["mcp"]));
        assert_eq!(server["env"]["DAILYBRIEF_RUN_ID"], "2026-09-17-abcd1234");
        assert_eq!(server["env"]["DAILYBRIEF_RUN_ROLE"], "editor");
        assert_eq!(
            server["env"]["DAILYBRIEF_CONFIG"],
            "/app/config/config.toml"
        );
        assert_eq!(server["env"]["DAILYBRIEF_DATA_DIR"], "/data");
        assert!(!out.contains("${"));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn mcp_json_carries_the_role() {
        let cmd = PathBuf::from("/usr/local/bin/dailybrief");
        let args = vec!["mcp".to_string()];
        let cfg = PathBuf::from("c.toml");
        let data = PathBuf::from("d");
        let mut v = vars(&cmd, &args, &cfg, &data);
        v.role = "curator";
        let out = render(TEMPLATE, &v).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["mcpServers"]["dailybrief"]["env"]["DAILYBRIEF_RUN_ROLE"],
            "curator"
        );
    }

    #[test]
    fn rendered_config_escapes_paths_with_quotes_and_flags_leftovers() {
        let cmd = PathBuf::from("/tmp/odd \"name\"/dailybrief");
        let args = vec!["mcp".to_string(), "--verbose".to_string()];
        let cfg = PathBuf::from("c.toml");
        let data = PathBuf::from("d");
        let out = render(TEMPLATE, &vars(&cmd, &args, &cfg, &data)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["mcpServers"]["dailybrief"]["command"],
            "/tmp/odd \"name\"/dailybrief"
        );
        assert_eq!(
            v["mcpServers"]["dailybrief"]["args"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let err = render("{\"x\": \"${UNKNOWN}\"}", &vars(&cmd, &args, &cfg, &data)).unwrap_err();
        assert!(
            matches!(err, McpConfigError::Unsubstituted(ref p) if p == "${UNKNOWN}"),
            "{err}"
        );
    }
}
