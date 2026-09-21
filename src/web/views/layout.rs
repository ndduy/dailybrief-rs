//! The page shell: mobile-first, system fonts, inline CSS, htmx (for the Refresh button) only.

use maud::{DOCTYPE, Markup, html};

/// htmx pinned from cdnjs; the only client-side script on the page. The integrity hash is
/// cdnjs's published SRI for this exact file; the CSP in `web::app` allows scripts from
/// cdnjs only, so a CDN compromise cannot serve a modified file.
pub const HTMX_SRC: &str = "https://cdnjs.cloudflare.com/ajax/libs/htmx/2.0.4/htmx.min.js";
pub const HTMX_SRI: &str = "sha512-2kIcAizYXhIn8TzUvqzEDZNuDZ+aW7yE/+f1HJHXFjQcGNfv1kqzJSTBRBSlOgp6B/KZsz1K0a3ZTqP9dnxioQ==";

const CSS: &str = r#"
:root{color-scheme:light dark;--fg:#1a1a1a;--bg:#fff;--muted:#666;--line:#e5e5e5;--badge:#eef}
@media(prefers-color-scheme:dark){:root{--fg:#eee;--bg:#111;--muted:#aaa;--line:#333;--badge:#223}}
*{box-sizing:border-box}
body{margin:0;padding:0 16px 48px;font:16px/1.5 system-ui,-apple-system,"Segoe UI",sans-serif;color:var(--fg);background:var(--bg)}
header{display:flex;justify-content:space-between;align-items:baseline;gap:1rem;padding:1rem 0;border-bottom:1px solid var(--line)}
h1{font-size:1.25rem;margin:0}h2{font-size:1rem;margin:1.5rem 0 .5rem;color:var(--muted);text-transform:uppercase;letter-spacing:.04em}
article{padding:.75rem 0;border-bottom:1px solid var(--line)}
article h3{font-size:1.05rem;margin:0 0 .25rem}article h3 a{color:inherit;text-decoration:none}article h3 a:hover{text-decoration:underline}
.meta{font-size:.85rem;color:var(--muted)}.why{font-style:italic;margin:.25rem 0 0}
.badge{display:inline-block;font-size:.75rem;padding:0 .4rem;border-radius:.5rem;background:var(--badge);margin-left:.4rem}
.state{padding:2rem 0;text-align:center;color:var(--muted)}
button{font:inherit;padding:.4rem .9rem;border:1px solid var(--line);border-radius:.4rem;background:var(--bg);color:var(--fg)}
pre{white-space:pre-wrap;word-break:break-word;font-size:.85rem}
.actions{display:flex;gap:.75rem;align-items:center}.actions a{color:var(--muted)}
"#;

pub fn page(title: &str, body: Markup) -> Markup {
    page_with_css(title, "", body)
}

/// The shell plus page-specific CSS; the digest page stays under its size cap by carrying
/// only the shared rules.
pub fn page_with_css(title: &str, extra_css: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="referrer" content="no-referrer";
                title { (title) }
                style { (maud::PreEscaped(CSS)) (maud::PreEscaped(extra_css)) }
                script src=(HTMX_SRC) integrity=(HTMX_SRI) crossorigin="anonymous" defer {}
            }
            body { (body) }
        }
    }
}

/// The header actions: the runs index and the Refresh button. Through htmx the `POST /run`
/// answer carries `HX-Refresh: true`, so the page reloads into the running state with no
/// inline script (the CSP has no `unsafe-eval` for htmx's `hx-on`).
pub fn refresh_button() -> Markup {
    html! {
        div.actions {
            a href="/runs" { "Runs" }
            form method="post" action="/run" hx-post="/run" hx-swap="none" {
                button type="submit" { "Refresh" }
            }
        }
    }
}

/// The header actions without the Refresh button (a run is already going).
pub fn runs_link() -> Markup {
    html! { div.actions { a href="/runs" { "Runs" } } }
}
