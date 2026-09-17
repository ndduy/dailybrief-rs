//! The three pages shown when there is no digest for a day: failed, running, none.

use maud::{Markup, html};

use super::layout::{page, refresh_button};
use crate::db::repo::RunRow;

pub fn failed(date: &str, run: &RunRow) -> Markup {
    page(
        &format!("No digest · {date}"),
        html! {
            header { h1 { "Daily Brief " span.meta { (date) } } (refresh_button()) }
            main.state {
                p { "No digest — run failed." }
                @if let Some(e) = &run.error { pre { (e) } }
                p.meta {
                    "Run " (run.id) " (" (run.status.as_str()) ", attempt " (run.attempt) ") · "
                    a href={ "/runs/" (run.id) "/transcript" } { "transcript" }
                }
            }
        },
    )
}

pub fn running(date: &str, run: &RunRow) -> Markup {
    page(
        &format!("Run in progress · {date}"),
        html! {
            header { h1 { "Daily Brief " span.meta { (date) } } }
            main.state {
                p { "Run in progress." }
                p.meta { "Run " (run.id) " started " (run.started_at) "." }
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
