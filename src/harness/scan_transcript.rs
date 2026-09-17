//! Egress scan of a run transcript (`CONSTRAINTS.md`): nothing but public article text and topic
//! names may have reached the model. Findings are one line each; an empty list is a clean run.

use std::collections::HashMap;

use serde_json::Value;

use crate::config::Topic;
use crate::db::{Connection, DbError, repo};

/// Fragments that must never appear in a transcript: vault paths and credentials.
pub const DEFAULT_FORBIDDEN: [&str; 5] = [
    "/vault",
    "sk-ant-",
    "OAUTH_TOKEN=",
    "ANTHROPIC_API_KEY",
    "Obsidian",
];
/// Topic descriptions shorter than this are too generic to be a meaningful leak signal.
const MIN_DESCRIPTION_CHARS: usize = 12;
pub const READ_ITEM_TOOL: &str = "mcp__dailybrief__read_item";
pub const WEB_SEARCH_TOOL: &str = "WebSearch";

pub struct ScanOptions<'a> {
    pub topics: &'a [Topic],
    /// Extra forbidden fragments (hostnames, paths) from the operator.
    pub forbidden: &'a [String],
    pub web_search_cap: u32,
}

/// Scans transcript lines; `conn` lets `read_item` results be checked against stored item text.
pub fn scan_transcript(
    lines: &[String],
    opts: &ScanOptions<'_>,
    conn: &Connection,
) -> Result<Vec<String>, DbError> {
    let mut findings = Vec::new();
    let forbidden: Vec<&str> = DEFAULT_FORBIDDEN
        .iter()
        .copied()
        .chain(opts.forbidden.iter().map(String::as_str))
        .collect();
    let descriptions: Vec<(&str, &str)> = opts
        .topics
        .iter()
        .map(|t| (t.name.as_str(), t.description.trim()))
        .filter(|(_, d)| d.chars().count() >= MIN_DESCRIPTION_CHARS)
        .collect();
    let mut uses: HashMap<String, String> = HashMap::new();
    let mut web_searches = 0u32;
    for (index, raw) in lines.iter().enumerate() {
        let n = index + 1;
        for f in &forbidden {
            if raw.contains(f) {
                findings.push(format!("line {n}: forbidden fragment '{f}'"));
            }
        }
        for (name, d) in &descriptions {
            if raw.contains(d) {
                findings.push(format!("line {n}: topic description of '{name}' leaked"));
            }
        }
        let Ok(event) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let blocks = event
            .pointer("/message/content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if kind == "assistant" {
            for b in &blocks {
                if b.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                let id = b.get("id").and_then(Value::as_str).unwrap_or_default();
                let name = b.get("name").and_then(Value::as_str).unwrap_or_default();
                uses.insert(id.to_string(), name.to_string());
                if name == WEB_SEARCH_TOOL {
                    web_searches += 1;
                }
            }
        }
        if kind == "user" {
            for b in &blocks {
                findings.extend(check_read_item(b, &uses, conn, n)?);
            }
        }
    }
    if web_searches > opts.web_search_cap {
        findings.push(format!(
            "{web_searches} WebSearch calls; the cap is {}",
            opts.web_search_cap
        ));
    }
    Ok(findings)
}

/// A `read_item` tool_result must carry the stored text of a known item; anything else is a finding.
fn check_read_item(
    block: &Value,
    uses: &HashMap<String, String>,
    conn: &Connection,
    n: usize,
) -> Result<Vec<String>, DbError> {
    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
        return Ok(Vec::new());
    }
    let use_id = block
        .get("tool_use_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if uses.get(use_id).map(String::as_str) != Some(READ_ITEM_TOOL) {
        return Ok(Vec::new());
    }
    if block.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Ok(Vec::new());
    }
    let text = result_text(block.get("content"));
    let json_start = text.find('{').unwrap_or(text.len());
    let Ok(payload) = serde_json::from_str::<Value>(&text[json_start..]) else {
        return Ok(vec![format!(
            "line {n}: read_item result could not be parsed"
        )]);
    };
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("?");
    let Some(item) = repo::get_item(conn, id)? else {
        return Ok(vec![format!(
            "line {n}: read_item result for unknown item '{id}'"
        )]);
    };
    if let Some(returned) = payload.get("text").and_then(Value::as_str)
        && !item.text.starts_with(returned)
    {
        return Ok(vec![format!(
            "line {n}: read_item text for '{id}' is not the stored article text"
        )]);
    }
    Ok(Vec::new())
}

/// tool_result content is either a string or an array of content blocks; join the text blocks.
fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TopicOrigin;
    use crate::core::testutil::{item, new_item, source};
    use crate::db::Db;

    const LEAKY: &str = include_str!("../../tests/fixtures/transcripts/leaky.jsonl");
    const CLEAN: &str = include_str!("../../tests/fixtures/transcripts/success.jsonl");

    fn topics() -> Vec<Topic> {
        vec![Topic {
            id: "rust".into(),
            name: "Rust".into(),
            description: "ownership, borrowing and lifetimes in systems code".into(),
            weight: 1.0,
            origin: TopicOrigin::Seed,
        }]
    }

    fn db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.with(|c| {
            source(c, "s")?;
            let mut known = new_item("known", "s", None, "2026-09-16T00:00:00.000Z");
            known.text = "The stored article text about ownership.".into();
            repo::insert_item(c, &known)?;
            item(c, "other", "s", None, "2026-09-16T00:00:00.000Z")
        })
        .unwrap();
        db
    }

    fn scan(text: &str, cap: u32) -> Vec<String> {
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let topics = topics();
        let extra = vec!["secret-host.example".to_string()];
        db().with(|c| {
            scan_transcript(
                &lines,
                &ScanOptions {
                    topics: &topics,
                    forbidden: &extra,
                    web_search_cap: cap,
                },
                c,
            )
        })
        .unwrap()
    }

    #[test]
    fn scan_flags_every_planted_leak() {
        let findings = scan(LEAKY, 5);
        let joined = findings.join("\n");
        assert!(joined.contains("forbidden fragment '/vault'"), "{joined}");
        assert!(
            joined.contains("forbidden fragment 'secret-host.example'"),
            "{joined}"
        );
        assert!(
            joined.contains("topic description of 'Rust' leaked"),
            "{joined}"
        );
        assert!(
            joined.contains("read_item text for 'known' is not the stored article text"),
            "{joined}"
        );
        assert!(
            joined.contains("read_item result for unknown item 'ghost'"),
            "{joined}"
        );
        assert!(
            joined.contains("6 WebSearch calls; the cap is 5"),
            "{joined}"
        );
        assert!(
            !joined.contains("line 9"),
            "an is_error read_item result is not a finding: {joined}"
        );
    }

    #[test]
    fn scan_passes_clean_transcript() {
        assert!(scan(CLEAN, 5).is_empty());
        let mut lenient = scan(LEAKY, 6);
        lenient.retain(|f| f.contains("WebSearch"));
        assert!(lenient.is_empty(), "6 searches pass under a cap of 6");
    }
}
