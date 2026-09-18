# ADR 0007: Article extraction is `dom_smoothie`, with `readability` as fallback

Date: 2026-09-18 · Status: accepted · Milestone: R0 (Task 7)

## Context
Article pages must become clean text before embedding and reading. `dom_smoothie` 0.18 is a Readability port with title, byline, text and word count; `readability` 0.3 is an older port kept as a second opinion.

## Decision
`core::extract` runs `dom_smoothie` in formatted text mode and falls back to `readability` when the first yields nothing. Text is whitespace-normalised, words are `split_whitespace()` tokens, and a page with fewer than 20 words extracts to `None`, which ingest skips. The `readability` crate is pulled without its default `reqwest` feature so the dependency graph stays on one TLS stack.

## Consequences
- Fixtures are three hand-written pages (long with nav/sidebar/footer tokens that must not leak, short, links-only); no external sample is imported.
- Extraction quality is judged on real feeds at first deployment; a page that extracts badly is a Curator `report_feed_issue`, not a code path.
