# dailybrief-rs — CLAUDE.md

Personal daily-reading agent, **Rust port** of the TypeScript `dailybrief` service (`../dailybrief`, which shipped M1 on 2026-09-17 and is the **reference implementation** and rollback target). An **Editor agent** (Claude Code headless, Max subscription) builds a 30-item digest every morning from RSS/Atom sources through an **MCP tool server** this repo provides (rmcp); a weekly **Curator agent** proposes profile changes behind an approval queue. Rust, axum, rusqlite, local embeddings. Full specification: **`SPEC.md`** — read it before doing anything.

## Mandated method — addyosmani/agent-skills
Every milestone runs through the toolkit loop: `/spec` → `/plan` → `/build` → `/test` → `/review` → `/ship`. Skill-to-section mapping and non-negotiables are in `SPEC.md` §0. Do not start implementation without an approved plan for the current milestone. Use the `code-reviewer` and `security-auditor` personas before each `/ship`.

## Current milestone
**R0** (`SPEC.md` §8): port M1 in full against the snapshot of the live database, gated by differential tests against the TypeScript build. Milestone spec to be written at `spec/r0.md` by `/spec`; module order for `/plan` is `SPEC.md` §7b. Nothing is built yet.

## Fixtures (already captured — the R0 correctness target, `tests/fixtures/`)
- `brief.db` — online backup of the live TS database taken 2026-09-17 (457 items, all with vectors, 7 digests, 12 runs). Migration ledger table: `migrations`. **Copy the DDL from `../dailybrief/src/db/migrations.ts`; never re-author it.**
- `mcp-tools-list.json` — `tools/list` result from the TS server; the Rust server's listing must match after key sort.
- `candidates-ts.json` — `get_briefing` and `list_candidates` (exploit 80 / popular_unmatched 30 / cold_topic 10) from the TS server **against this same `brief.db`**.
- `vectors-200.json` — 200 items with text and the stored 384-d vectors; Rust `fastembed` BGESmallENV15 must reach cosine ≥ 0.99, else the re-embed ADR.
- `html/<id>.html` + `pages.json` + `extraction-ts.json` — 20 public article pages and the TS extraction of each (Defuddle → Readability); Rust `dom_smoothie` must land within ±15 % word count on ≥ 17/20.
- `runs/<id>/` — three real `stream-json` transcripts (a 61-turn `error_max_turns` failure, its 92-turn `success` retry, and a killed run with no `result` line) plus the rendered `mcp.json` the harness saw; the source of truth for the `serde` event enum and the egress scan. See `tests/fixtures/README.md`.

## Hard constraints (from SPEC.md §1, §2, §7, §11)
- **Subscription billing only.** Production runs are `claude -p` (never `--bare`) inside the service container, authenticated by a `claude setup-token` OAuth token (`CLAUDE_CODE_OAUTH_TOKEN` in the gitignored `.env`). **Never put `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` in the service environment, `.env`, compose file, Dockerfile, systemd unit, or shell profile.** The M6 side-quest key lives in a separate shell only.
- **Egress.** Only public article text and topic *names / one-line descriptions* may reach any model or web API. Embeddings are computed locally. No vault content is read at runtime.
- **No deterministic fallback.** A failed run is retried once, then shown as failed with its transcript.
- **Every state change beyond today's digest goes through the approval queue** (`propose_change` only writes a proposal row).
- Service binds `127.0.0.1:8788` only; Cloudflare Access is the trust boundary (JWT middleware is in R0, not M5).
- **Same SQLite schema as the TS build.** Vectors are little-endian `f32` blobs.
- Secrets never enter the repo or an image layer. `config.toml`, `feeds.toml`, `topics.toml` are committed; `.env` and `data/` are not.

## Rust rules (SPEC.md §7a)
- No `unsafe`. No `unwrap()` / `expect()` outside `#[cfg(test)]` and `main.rs`. `cargo clippy --all-targets -- -D warnings` clean.
- `thiserror` enums in library modules, `anyhow` only in `main`. Tool errors are typed variants rendered to short, stable messages the prompt can quote.
- All SQL in `db::repo`; rusqlite, fastembed and dom_smoothie calls run inside `tokio::task::spawn_blocking`.
- Contract types (`schemars` + `serde`, `rename_all = "camelCase"`) are the MCP tool schemas; `cargo run --bin gen-schemas -- --check` keeps `schemas/digest.json` in sync (draft-07).
- **Verify every crate API on docs.rs for the resolved version before using it** (`SPEC.md` marks these *verify at build time*).

## Quality bar
`CONSTRAINTS.md` is written during `/spec` R0 from `SPEC.md` §7c and §11. Read it before writing code. Do not weaken it to make a change pass.

## Conventions
- Everything runs in Docker once the image exists (R0 task): `bin/dc cargo <cmd>` for every check and test, `docker compose --profile app up` for the service. Until then, host `cargo` (1.95, `rust-toolchain.toml`) is acceptable for the first slices, and the Docker task must land before the first `/ship`.
- Edition 2024. Trunk-based on `main`, atomic commits per task, ADR per decision in `docs/adr/` (start at 0001; the TS repo's ADRs are referenced by number as "TS ADR 000N").
- Tests are required for every task (toolkit rule); `cargo test` must pass before `/ship`.
- Verify harness CLI flags against `claude --help` / official docs before relying on them.
