//! The digest page: date header, `for_you` then `beyond_radar`, one card per item.

use maud::{Markup, html};

use super::layout::{page, refresh_button};
use crate::db::repo::{DigestCard, DigestRow, Section};

fn short_date(iso: &str) -> &str {
    iso.get(..10).unwrap_or(iso)
}

fn card(c: &DigestCard) -> Markup {
    html! {
        article {
            h3 { a href={ "/r/" (c.item_id) } { (c.title) } }
            p.meta {
                (c.source)
                @if let Some(p) = &c.published_at { " · " (short_date(p)) }
                " · " (c.topic)
                @if let Some(r) = &c.reason { span.badge { (r) } }
            }
            p { (c.summary) }
            p.why { (c.why_it_matters) }
        }
    }
}

pub fn render(digest: &DigestRow, cards: &[DigestCard]) -> Markup {
    let for_you: Vec<&DigestCard> = cards
        .iter()
        .filter(|c| c.section == Section::ForYou)
        .collect();
    let beyond: Vec<&DigestCard> = cards
        .iter()
        .filter(|c| c.section == Section::BeyondRadar)
        .collect();
    let title = format!("Daily Brief · {}", digest.date);
    page(
        &title,
        html! {
            header {
                h1 { "Daily Brief " span.meta { (digest.date) } }
                (refresh_button())
            }
            main {
                h2 { "For you" }
                @for c in &for_you { (card(c)) }
                h2 { "Beyond your radar" }
                @for c in &beyond { (card(c)) }
            }
        },
    )
}
