//! The rating widget on a digest card (`spec/m3.md` rating-ui): two buttons, the four reasons
//! for the chosen sign revealed inline, one tap completes, an undo. htmx attributes only, no
//! inline script (the CSP has no `unsafe-eval`); every form also works without htmx.
//!
//! Size: 30 widgets must fit the 24 KiB page cap, so the card slot carries only the item id
//! (the server records the newest digest that item was in) and inherits `hx-target` /
//! `hx-swap` from the page's `main` ([`TARGET_ATTRS`]); the partials, which arrive alone,
//! carry their own.

use maud::{Markup, html};

use crate::core::feedback::{Reason, Sign};
use crate::db::repo::RatingRow;

/// Page-specific CSS for the widget; the digest page injects it once.
pub fn css() -> &'static str {
    ".rate{display:flex;flex-wrap:wrap;gap:.4rem;align-items:center;margin-top:.4rem;font-size:.85rem}\
     .rate form{display:contents}.rate button{padding:.15rem .6rem}.rate .picked{color:var(--muted)}"
}

/// `hx-target` and `hx-swap` for every widget, set once on the digest page's `main`.
pub const TARGET: &str = "closest .rate";
pub const SWAP: &str = "outerHTML";

/// The slot as it renders on the card: rated (choice + undo) or the two sign buttons.
pub fn slot(item_id: &str, rating: Option<&RatingRow>) -> Markup {
    match rating {
        Some(r) => rated(item_id, r),
        None => unrated(item_id),
    }
}

fn unrated(item_id: &str) -> Markup {
    html! {
        div.rate {
            form action="/rate/reasons" hx-get="/rate/reasons" {
                input type="hidden" name="item" value=(item_id);
                button name="sign" value="up" { "👍" }
                button name="sign" value="down" { "👎" }
            }
        }
    }
}

fn rated(item_id: &str, r: &RatingRow) -> Markup {
    let sign = Sign::parse(&r.sign).map(glyph).unwrap_or("?");
    let label = Reason::parse(&r.reason)
        .map(Reason::label)
        .unwrap_or(r.reason.as_str());
    html! {
        div.rate {
            span.picked { (sign) " " (label) }
            form method="post" action="/rate" hx-post="/rate" hx-target=(TARGET) hx-swap=(SWAP) {
                input type="hidden" name="item" value=(item_id);
                input type="hidden" name="sign" value="none";
                button { "Undo" }
            }
        }
    }
}

fn glyph(sign: Sign) -> &'static str {
    match sign {
        Sign::Up => "👍",
        Sign::Down => "👎",
    }
}

/// The four reasons for one sign, each a one-tap submit, plus a way back.
pub fn reasons(item_id: &str, sign: Sign) -> Markup {
    html! {
        div.rate {
            span.picked { (glyph(sign)) }
            form method="post" action="/rate" hx-post="/rate" hx-target=(TARGET) hx-swap=(SWAP) {
                input type="hidden" name="item" value=(item_id);
                input type="hidden" name="sign" value=(sign.as_str());
                @for r in Reason::for_sign(sign) {
                    button name="reason" value=(r.as_str()) { (r.label()) }
                }
            }
            form action="/rate/reasons" hx-get="/rate/reasons" hx-target=(TARGET) hx-swap=(SWAP) {
                input type="hidden" name="item" value=(item_id);
                input type="hidden" name="sign" value="none";
                button { "Cancel" }
            }
        }
    }
}
