//! The digest page: date header, `for_you` then `beyond_radar`, one card per item.

use std::collections::HashMap;

use maud::{Markup, html};

use super::layout::{page_with_css, refresh_button};
use super::rate;
use crate::db::repo::{DigestCard, DigestRow, RatingRow, Section};

fn short_date(iso: &str) -> &str {
    iso.get(..10).unwrap_or(iso)
}

fn card(c: &DigestCard, rating: Option<&RatingRow>) -> Markup {
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
            (rate::slot(&c.item_id, rating))
        }
    }
}

/// `ratings` is keyed by item id (at most one per item).
pub fn render(digest: &DigestRow, cards: &[DigestCard], ratings: &[RatingRow]) -> Markup {
    let by_item: HashMap<&str, &RatingRow> =
        ratings.iter().map(|r| (r.item_id.as_str(), r)).collect();
    let card = |c: &DigestCard| card(c, by_item.get(c.item_id.as_str()).copied());
    let for_you: Vec<&DigestCard> = cards
        .iter()
        .filter(|c| c.section == Section::ForYou)
        .collect();
    let beyond: Vec<&DigestCard> = cards
        .iter()
        .filter(|c| c.section == Section::BeyondRadar)
        .collect();
    let title = format!("Daily Brief · {}", digest.date);
    page_with_css(
        &title,
        rate::css(),
        html! {
            header {
                h1 { "Daily Brief " span.meta { (digest.date) } }
                (refresh_button())
            }
            // The rating widgets inherit their htmx target and swap from here (size cap).
            main hx-target=(rate::TARGET) hx-swap=(rate::SWAP) {
                h2 { "For you" }
                @for c in &for_you { (card(c)) }
                h2 { "Beyond your radar" }
                @for c in &beyond { (card(c)) }
            }
        },
    )
}
