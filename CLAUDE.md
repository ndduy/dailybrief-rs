# dailybrief-rs — CLAUDE.md

Personal daily-reading agent in **Rust**. An **Editor agent** (Claude Code headless, Max subscription) builds a 30-item digest every morning from RSS/Atom sources through an **MCP tool server** this repo provides (rmcp); a weekly **Curator agent** proposes profile changes behind an approval queue. Rust, axum, rusqlite, local embeddings. Full specification: **`SPEC.md`** — read it before doing anything.

## Clean build from the spec
`SPEC.md` is self-contained: the SQLite DDL (§5), the tool contracts (§4), the scoring rules (§5), the harness flags and runner policy (§3), the web routes (§7b) and the quality gates (§7c) are all written out. Build from it and nothing else — do not look for another implementation to port from or diff against. If something needed is missing from `SPEC.md`, say so and ask. The committed inputs (`prompts/`, `schemas/`, `config/` — `config.toml`, `feeds.toml`, `topics.toml`, `mcp.json` — and `docs/references/`) are inputs; only `schemas/digest.json` is regenerated, from the Rust contract type.

## Mandated method — addyosmani/agent-skills
Every milestone runs through the toolkit loop: `/spec` → `/plan` → `/build` → `/test` → `/review` → `/ship`. Skill-to-section mapping and non-negotiables are in `SPEC.md` §0. Do not start implementation without an approved plan for the current milestone. Use the `code-reviewer` and `security-auditor` personas before each `/ship`.

## Current milestone
**M2** (`SPEC.md` §8): trajectories on the page and the R0 review backlog. Milestone spec at `spec/m2.md` (capability map approved 2026-09-21); plan goes to `tasks/plan.md` by `/plan`. R0 shipped 2026-09-21 (`docs/ship/r0.md`, `spec/r0.md`); the service runs from `dailybrief-rs:runtime` on the box, and M2 deploys only between mornings.

## Test material (all produced in this repo; nothing imported)
- `tests/fixtures/feeds/` — small hand-written RSS 2.0 and Atom files.
- `tests/fixtures/html/` — a few sample article pages written for the extraction tests (long, short, no-body).
- `tests/fixtures/transcripts/` — `stream-json` files: hand-written minimal success / `error_max_turns` / no-`result` cases first, then the real transcript of this repo's first smoke run once it exists.
- `tests/fake-claude/` — an executable that replays a transcript file; adapter tests put it on `PATH`.
- Unit tests use an in-memory SQLite migrated with the real ledger and a deterministic fake `Embedder`; the ONNX model loads only in the opt-in `EMBED_REAL=1` test.

## Hard constraints (from SPEC.md §1, §2, §7, §11)
- **Subscription billing only.** Production runs are `claude -p` (never `--bare`) inside the service container, authenticated by a `claude setup-token` OAuth token (`CLAUDE_CODE_OAUTH_TOKEN` in the gitignored `.env`). **Never put `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN` in the service environment, `.env`, compose file, Dockerfile, systemd unit, or shell profile.** The M6 side-quest key lives in a separate shell only.
- **Egress.** Only public article text and topic *names / one-line descriptions* may reach any model or web API. Embeddings are computed locally. No vault content is read at runtime.
- **No deterministic fallback.** A failed run is retried once, then shown as failed with its transcript.
- **Every state change beyond today's digest goes through the approval queue** (`propose_change` only writes a proposal row).
- Service binds `127.0.0.1:8788` only; Cloudflare Access is the trust boundary (JWT middleware is in R0, not M5).
- **SQLite schema exactly as `SPEC.md` §5**, migration ledger `migrations(id, applied_at)`, `0001_init` already applied in the existing `brief.db` that the service adopts on first deployment. Vectors are little-endian `f32` blobs; they are re-embedded once then (`dailybrief reembed`).
- Secrets never enter the repo or an image layer. `config/*.toml` and `config/mcp.json` are committed; `.env` and `data/` are not.

## Rust rules (SPEC.md §7a)
- No `unsafe`. No `unwrap()` / `expect()` outside `#[cfg(test)]` and `main.rs`. `cargo clippy --all-targets -- -D warnings` clean.
- `thiserror` enums in library modules, `anyhow` only in `main`. Tool errors are typed variants rendered to short, stable messages the prompt can quote.
- All SQL in `db::repo`; rusqlite, fastembed and dom_smoothie calls run inside `tokio::task::spawn_blocking`.
- Contract types (`schemars` + `serde`, `rename_all = "camelCase"`) are the MCP tool schemas; `cargo run --bin gen-schemas -- --check` keeps `schemas/digest.json` in sync (draft-07).
- **Verify every crate API on docs.rs for the resolved version before using it** (`SPEC.md` marks these *verify at build time*).

## Quality bar
`CONSTRAINTS.md` is written during `/spec` R0 from `SPEC.md` §7c and §11. Read it before writing code. Do not weaken it to make a change pass.

## Conventions
- Everything runs in Docker once the image exists (R0 task): `bin/dc cargo <cmd>` for every check and test, `docker compose --profile app up` for the service. Until then, host `cargo` (1.98, `rust-toolchain.toml`) is acceptable for the first slices, and the Docker task must land before the first `/ship`.
- Edition 2024. Trunk-based on `main`, atomic commits per task, ADR per decision in `docs/adr/` (start at 0001).
- Tests are required for every task (toolkit rule); `cargo test` must pass before `/ship`.
- Verify harness CLI flags against `claude --help` / official docs before relying on them.
