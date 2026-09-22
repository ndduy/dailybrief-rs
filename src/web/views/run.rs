//! The run page (`/runs/{id}`, ADR 0012) and the runs index: the caps bar, the retry chain and
//! one row per turn, rendered over `harness::trajectory` types. The raw event page
//! (`/runs/{id}/log`) and the transcript download stay one link away.

use maud::{Markup, html};

use super::layout::{page_with_css, runs_link};
use crate::core::time::parse_iso;
use crate::db::repo::{RunRow, RunStatus};
use crate::harness::trajectory::{AttemptOutcome, CapsUsed, Turn, Used};

/// `SPEC.md` §3: one retry, so every run is attempt 1 or 2 of 2.
pub const MAX_ATTEMPTS: u32 = 2;

/// The caps bar's styles: shared by the run pages and the day page's failed and running
/// states (injected per page; the digest page keeps its 15 KiB cap).
pub const CAPS_CSS: &str = r#"
.caps{display:flex;flex-wrap:wrap;gap:.5rem;margin:.75rem 0}.cap{font-size:.85rem;padding:.1rem .5rem;border:1px solid var(--line);border-radius:1rem;white-space:nowrap}.cap.hit{border-color:#b32424;color:#b32424;font-weight:600}
@media(prefers-color-scheme:dark){.cap.hit{border-color:#f66;color:#f66}}
"#;

/// Styles only the run pages need.
const CSS: &str = r#"
table{border-collapse:collapse;width:100%;font-size:.9rem}td,th{text-align:left;padding:.35rem .5rem;border-bottom:1px solid var(--line);vertical-align:top}td.num,th.num{text-align:right;white-space:nowrap}
.chain{margin:.5rem 0;padding-left:1.2rem}.chain li{margin:.15rem 0}
.turns td.tool{white-space:nowrap}.turns td.args{color:var(--muted);font-family:ui-monospace,monospace;font-size:.8rem;word-break:break-all}.turns td.n{color:var(--muted)}.turns .err{color:#b32424;font-weight:600}
.status-success{color:#1a7a4a}.status-failed,.status-killed{color:#b32424}.status-running{color:#9a5b00}
@media(prefers-color-scheme:dark){.status-success{color:#5c9}.status-failed,.status-killed{color:#f66}.status-running{color:#fb4}}
"#;

/// `500` → `8:20`.
pub fn mmss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn status_class(status: RunStatus) -> String {
    format!("status-{}", status.as_str())
}

fn cap(label: &str, u: &Used) -> Markup {
    html! {
        span class=(if u.hit { "cap hit" } else { "cap" }) { (label) " " (u.used) "/" (u.cap) }
    }
}

/// `reads 32/45 · selects 31/30 · web 0/5 · turns 77/120 · 8:20/15:00 · attempt 1 of 2`.
pub fn caps_bar(c: &CapsUsed, attempt: i64) -> Markup {
    html! {
        div.caps {
            (cap("reads", &c.reads))
            (cap("selects", &c.selects))
            (cap("web", &c.searches))
            (cap("turns", &c.turns))
            span class=(if c.wall_secs.hit { "cap hit" } else { "cap" }) {
                (mmss(c.wall_secs.used)) "/" (mmss(c.wall_secs.cap))
            }
            span.cap { "attempt " (attempt) " of " (MAX_ATTEMPTS) }
        }
    }
}

fn chain_list(chain: &[AttemptOutcome], current: &str) -> Markup {
    html! {
        @if chain.len() > 1 {
            ol.chain {
                @for a in chain {
                    li {
                        "attempt " (a.attempt) ": "
                        span class=(status_class(a.status)) { (a.status.as_str()) }
                        @if let Some(e) = &a.error { " — " (e) }
                        @if a.run_id != current { " · " a href={ "/runs/" (a.run_id) } { (a.run_id) } }
                    }
                }
            }
        }
    }
}

fn turn_row(t: &Turn) -> Markup {
    html! {
        tr.turn {
            td.n { (t.n) }
            td.num {
                @if let Some(s) = t.since_prev_secs { "+" (format!("{s:.1}")) "s" }
            }
            td.tool {
                @if let Some(tool) = &t.tool { (tool) } @else { span.meta { "text" } }
            }
            td.args {
                @if t.tool.is_some() { (t.args_summary) } @else { (t.text_preview) }
            }
            td.num {
                @if t.result_chars > 0 { (t.result_chars) }
                @if t.errors > 0 { " " span.err { "✗" (t.errors) } }
            }
            td.num { (t.tokens.output) }
            td.num { (t.tokens.cache_read) }
        }
    }
}

/// `/runs/{id}`. A running run's page reloads itself every 15 s.
pub fn render_run(
    run: &RunRow,
    chain: &[AttemptOutcome],
    caps: &CapsUsed,
    turns: &[Turn],
) -> Markup {
    let title = format!("Run {}", run.id);
    let body = html! {
        @if run.status == RunStatus::Running { meta http-equiv="refresh" content="15"; }
        header { h1 { "Run " span.meta { (run.id) } } (runs_link()) }
        main {
            p.meta {
                span class=(status_class(run.status)) { (run.status.as_str()) }
                " · " (run.kind) " · started " (run.started_at)
                @if let Some(e) = &run.ended_at { " · ended " (e) }
                " · " a href={ "/runs/" (run.id) "/log" } { "log" }
                " · " a href={ "/runs/" (run.id) "/transcript" } { "transcript" }
            }
            @if let Some(e) = &run.error { pre { (e) } }
            (caps_bar(caps, run.attempt))
            (chain_list(chain, &run.id))
            @if turns.is_empty() { p.state { "No turns yet." } }
            @else {
                table.turns {
                    thead { tr {
                        th { "#" } th.num { "+s" } th { "tool" } th { "args / text" }
                        th.num { "chars back" } th.num { "out tok" } th.num { "cache rd" }
                    } }
                    tbody { @for t in turns { (turn_row(t)) } }
                }
            }
        }
    };
    page_with_css(&title, &format!("{CAPS_CSS}{CSS}"), body)
}

fn wall_of(r: &RunRow) -> Option<String> {
    let started = parse_iso(&r.started_at)?;
    let ended = parse_iso(r.ended_at.as_deref()?)?;
    Some(mmss((ended - started).num_seconds().max(0) as u64))
}

/// `/runs`: the most recent runs, newest first, each linking to its page and raw log.
pub fn render_index(runs: &[RunRow]) -> Markup {
    page_with_css(
        "Runs",
        &format!("{CAPS_CSS}{CSS}"),
        html! {
            header { h1 { "Runs" } (runs_link()) }
            main {
                @if runs.is_empty() { p.state { "No runs yet." } }
                @else {
                    table {
                        thead { tr {
                            th { "Run" } th { "Status" } th { "Kind" } th { "Started" }
                            th.num { "Turns" } th.num { "Wall" } th { "Error" }
                        } }
                        tbody {
                            @for r in runs {
                                tr {
                                    td {
                                        a href={ "/runs/" (r.id) } { (r.id) }
                                        " · " a href={ "/runs/" (r.id) "/log" } { "log" }
                                    }
                                    td { span class=(status_class(r.status)) { (r.status.as_str()) } }
                                    td { (r.kind) " #" (r.attempt) }
                                    td { (r.started_at) }
                                    td { @if let Some(t) = r.turns { (t) } }
                                    td { @if let Some(w) = wall_of(r) { (w) } }
                                    td { @if let Some(e) = &r.error { (e.chars().take(120).collect::<String>()) } }
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
    fn mmss_formats_minutes_and_zero_padded_seconds() {
        assert_eq!(mmss(0), "0:00");
        assert_eq!(mmss(500), "8:20");
        assert_eq!(mmss(900), "15:00");
        assert_eq!(mmss(3661), "61:01");
    }

    #[test]
    fn caps_bar_marks_only_hit_caps() {
        let c = CapsUsed {
            selects: Used {
                used: 31,
                cap: 30,
                hit: true,
            },
            reads: Used {
                used: 3,
                cap: 45,
                hit: false,
            },
            ..CapsUsed::default()
        };
        let html = caps_bar(&c, 2).into_string();
        assert!(html.contains("<span class=\"cap hit\">selects 31/30</span>"));
        assert!(html.contains("<span class=\"cap\">reads 3/45</span>"));
        assert!(html.contains("attempt 2 of 2"));
    }
}
