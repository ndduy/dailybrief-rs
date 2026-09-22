//! The three pages shown when there is no digest for a day: failed, running, none.

use maud::{Markup, html};

use super::layout::{page, page_with_css, refresh_button, runs_link};
use super::run::{CAPS_CSS, caps_bar};
use crate::db::repo::RunRow;
use crate::harness::trajectory::CapsUsed;

pub fn failed(date: &str, run: &RunRow, caps: &CapsUsed) -> Markup {
    page_with_css(
        &format!("No digest · {date}"),
        CAPS_CSS,
        html! {
            header { h1 { "Daily Brief " span.meta { (date) } } (refresh_button()) }
            main.state {
                p { "No digest — run failed." }
                @if let Some(e) = &run.error { pre { (e) } }
                (caps_bar(caps, run.attempt))
                p.meta {
                    "Run " a href={ "/runs/" (run.id) } { (run.id) }
                    " (" (run.status.as_str()) ", attempt " (run.attempt) ") · "
                    a href={ "/runs/" (run.id) "/log" } { "log" } " · "
                    a href={ "/runs/" (run.id) "/transcript" } { "transcript" }
                }
            }
        },
    )
}

pub fn running(date: &str, run: &RunRow, caps: &CapsUsed) -> Markup {
    page_with_css(
        &format!("Run in progress · {date}"),
        CAPS_CSS,
        html! {
            header { h1 { "Daily Brief " span.meta { (date) } } (runs_link()) }
            main.state {
                p { "Run in progress." }
                (caps_bar(caps, run.attempt))
                p.meta {
                    "Run " a href={ "/runs/" (run.id) } { (run.id) } " started " (run.started_at) " · "
                    a href={ "/runs/" (run.id) "/log" } { "log" }
                }
            }
        },
    )
}

pub fn none(date: &str) -> Markup {
    page(
        &format!("No run yet · {date}"),
        html! {
            header { h1 { "Daily Brief " span.meta { (date) } } (refresh_button()) }
            main.state { p { "No run yet." } }
        },
    )
}

pub fn bad_date(input: &str) -> Markup {
    page(
        "Not a date",
        html! {
            main.state { p { "'" (input) "' is not a date. Use /d/YYYY-MM-DD." } }
        },
    )
}
