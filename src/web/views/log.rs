//! The run log pages: `/runs` (recent runs) and `/runs/{id}/log` (one run's events, one line per
//! `stream-json` event with a readable summary and the payload behind a disclosure). Built from
//! `run_events`, so it is the same data the egress scan and the transcript download see.

use maud::{Markup, html};
use serde_json::Value;

use super::layout::{page_with_css, runs_link};
use crate::db::repo::{RunEvent, RunRow, RunStatus};

/// Characters kept in a one-line summary.
const SUMMARY_CHARS: usize = 300;
/// Characters kept in the expanded payload.
const DETAIL_CHARS: usize = 4000;
/// Rows on the runs index.
pub const INDEX_LIMIT: i64 = 30;

/// Styles only these pages need (the digest page has a size cap).
const CSS: &str = r#"
table{border-collapse:collapse;width:100%;font-size:.9rem}td,th{text-align:left;padding:.4rem .5rem;border-bottom:1px solid var(--line);vertical-align:top}
.log{list-style:none;padding:0;margin:0}.log li{padding:.35rem 0;border-bottom:1px solid var(--line);font-size:.9rem}.log .seq{color:var(--muted);display:inline-block;min-width:3rem}.log .kind{display:inline-block;min-width:5.5rem;font-weight:600}.log details{margin-top:.2rem}.log summary{cursor:pointer;color:var(--muted)}.log pre{margin:.3rem 0 0;max-height:24rem;overflow:auto}
.status-success{color:#1a7a4a}.status-failed,.status-killed{color:#b32424}.status-running{color:#9a5b00}
@media(prefers-color-scheme:dark){.status-success{color:#5c9}.status-failed,.status-killed{color:#f66}.status-running{color:#fb4}}
"#;

fn truncate(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().nth(max).is_some() {
        out.push('…');
    }
    out
}

fn tool_name(name: &str) -> &str {
    name.strip_prefix("mcp__dailybrief__").unwrap_or(name)
}

/// The text inside a `tool_result` block: a string, or an array of `{type: text}` parts.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        other => other.to_string(),
    }
}

/// One readable line per event: what happened, in the words a tester looks for
/// ("→ read_item {…}", "← 5000 chars", "text: …", "init · 11 tools", "result · success").
pub fn summarize(kind: &str, payload_json: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(payload_json) else {
        return truncate(payload_json, SUMMARY_CHARS);
    };
    let blocks = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array);
    match kind {
        "assistant" | "user" => {
            let Some(blocks) = blocks else {
                return truncate(&v.to_string(), SUMMARY_CHARS);
            };
            let parts: Vec<String> = blocks
                .iter()
                .map(|b| match b.get("type").and_then(Value::as_str) {
                    Some("tool_use") => format!(
                        "→ {} {}",
                        tool_name(b.get("name").and_then(Value::as_str).unwrap_or("?")),
                        b.get("input").map(Value::to_string).unwrap_or_default()
                    ),
                    Some("tool_result") => format!(
                        "← {}",
                        b.get("content").map(result_text).unwrap_or_default()
                    ),
                    Some("text") => format!(
                        "text: {}",
                        b.get("text").and_then(Value::as_str).unwrap_or_default()
                    ),
                    Some(other) => other.to_string(),
                    None => b.to_string(),
                })
                .collect();
            truncate(&parts.join(" · "), SUMMARY_CHARS)
        }
        "system" => {
            let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("system");
            if subtype == "init" {
                let tools = v
                    .get("tools")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                let servers: Vec<String> = v
                    .get("mcp_servers")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|s| {
                                format!(
                                    "{} ({})",
                                    s.get("name").and_then(Value::as_str).unwrap_or("?"),
                                    s.get("status").and_then(Value::as_str).unwrap_or("?")
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                format!(
                    "init · model {} · {tools} tools · mcp: {}",
                    v.get("model").and_then(Value::as_str).unwrap_or("?"),
                    servers.join(", ")
                )
            } else {
                subtype.to_string()
            }
        }
        "result" => format!(
            "result · {} · {} turns · {} s",
            v.get("subtype").and_then(Value::as_str).unwrap_or("?"),
            v.get("num_turns").and_then(Value::as_i64).unwrap_or(0),
            v.get("duration_ms").and_then(Value::as_i64).unwrap_or(0) / 1000
        ),
        _ => truncate(&v.to_string(), SUMMARY_CHARS),
    }
}

fn pretty(payload_json: &str) -> String {
    let text = serde_json::from_str::<Value>(payload_json)
        .and_then(|v| serde_json::to_string_pretty(&v))
        .unwrap_or_else(|_| payload_json.to_string());
    truncate(&text, DETAIL_CHARS)
}

fn status_class(status: RunStatus) -> String {
    format!("status-{}", status.as_str())
}

fn run_meta(run: &RunRow) -> Markup {
    html! {
        p.meta {
            span class=(status_class(run.status)) { (run.status.as_str()) }
            " · " (run.kind) " · attempt " (run.attempt)
            " · started " (run.started_at)
            @if let Some(e) = &run.ended_at { " · ended " (e) }
            @if let Some(t) = run.turns { " · " (t) " turns" }
            " · " a href={ "/runs/" (run.id) "/transcript" } { "transcript" }
        }
        @if let Some(e) = &run.error { pre { (e) } }
    }
}

/// `/runs/{id}/log`. A running run's page reloads itself every 15 s.
pub fn render_log(run: &RunRow, events: &[RunEvent]) -> Markup {
    let title = format!("Run {} · log", run.id);
    let body = html! {
        header { h1 { "Run " span.meta { (run.id) } } (runs_link()) }
        main {
            (run_meta(run))
            @if events.is_empty() { p.state { "No events yet." } }
            ul.log {
                @for e in events {
                    li {
                        span.seq { (e.seq) }
                        span.kind { (e.kind) }
                        (summarize(&e.kind, &e.payload_json))
                        details { summary { "payload" } pre { (pretty(&e.payload_json)) } }
                    }
                }
            }
        }
    };
    if run.status == RunStatus::Running {
        // maud has no slot for extra head tags; the meta refresh is the first thing in the
        // body, which browsers honour just the same.
        page_with_css(
            &title,
            CSS,
            html! {
                meta http-equiv="refresh" content="15";
                (body)
            },
        )
    } else {
        page_with_css(&title, CSS, body)
    }
}

/// `/runs`: the most recent runs, newest first, each linking to its log.
pub fn render_index(runs: &[RunRow]) -> Markup {
    page_with_css(
        "Runs",
        CSS,
        html! {
            header { h1 { "Runs" } (runs_link()) }
            main {
                @if runs.is_empty() { p.state { "No runs yet." } }
                @else {
                    table {
                        thead { tr { th { "Run" } th { "Status" } th { "Kind" } th { "Started" } th { "Turns" } th { "Error" } } }
                        tbody {
                            @for r in runs {
                                tr {
                                    td { a href={ "/runs/" (r.id) "/log" } { (r.id) } }
                                    td { span class=(status_class(r.status)) { (r.status.as_str()) } }
                                    td { (r.kind) " #" (r.attempt) }
                                    td { (r.started_at) }
                                    td { @if let Some(t) = r.turns { (t) } }
                                    td { @if let Some(e) = &r.error { (truncate(e, 120)) } }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_tool_use_reads_as_an_arrow_with_the_short_tool_name() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"mcp__dailybrief__read_item","input":{"id":"abc"}}]}}"#;
        assert_eq!(summarize("assistant", line), r#"→ read_item {"id":"abc"}"#);
    }

    #[test]
    fn user_tool_result_reads_as_a_left_arrow_and_is_truncated() {
        let long = "x".repeat(SUMMARY_CHARS + 50);
        let line = format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","content":[{{"type":"text","text":"{long}"}}]}}]}}}}"#
        );
        let s = summarize("user", &line);
        assert!(s.starts_with("← xxx"));
        assert!(s.ends_with('…'));
        assert_eq!(s.chars().count(), SUMMARY_CHARS + 1);
    }

    #[test]
    fn assistant_text_and_mixed_blocks_join_with_a_dot() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Reading now."},{"type":"tool_use","name":"WebSearch","input":{"q":"rust"}}]}}"#;
        assert_eq!(
            summarize("assistant", line),
            r#"text: Reading now. · → WebSearch {"q":"rust"}"#
        );
    }

    #[test]
    fn init_counts_tools_and_names_servers_result_shows_subtype_turns_seconds() {
        let init = r#"{"type":"system","subtype":"init","model":"claude-opus-5","tools":["WebSearch","mcp__dailybrief__select"],"mcp_servers":[{"name":"dailybrief","status":"connected"}]}"#;
        assert_eq!(
            summarize("system", init),
            "init · model claude-opus-5 · 2 tools · mcp: dailybrief (connected)"
        );
        let result = r#"{"type":"result","subtype":"success","num_turns":84,"duration_ms":616298}"#;
        assert_eq!(
            summarize("result", result),
            "result · success · 84 turns · 616 s"
        );
    }

    #[test]
    fn unparseable_and_unknown_kinds_fall_back_to_the_raw_text() {
        assert_eq!(summarize("unparseable", "not json {"), "not json {");
        assert_eq!(
            summarize("rate_limit_event", r#"{"type":"rate_limit_event","x":1}"#),
            r#"{"type":"rate_limit_event","x":1}"#
        );
    }

    #[test]
    fn pretty_payload_is_indented_and_capped() {
        let p = pretty(r#"{"a":1}"#);
        assert_eq!(p, "{\n  \"a\": 1\n}");
        let big = format!(r#"{{"a":"{}"}}"#, "y".repeat(DETAIL_CHARS + 10));
        assert_eq!(pretty(&big).chars().count(), DETAIL_CHARS + 1);
    }
}
