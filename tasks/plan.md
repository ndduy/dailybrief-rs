# Implementation Plan: R0 — the Rust service, built in full

Source: `spec/r0.md` (approved 2026-09-17) over `SPEC.md` v4. Quality bar: `CONSTRAINTS.md`. Task list: `tasks/todo.md`. Status: **approved 2026-09-17.**

## Overview

Build the whole of M1 in Rust from the spec: config and migrations that adopt the existing `brief.db`, an ingest pipeline (feeds → extraction → dedupe → local embeddings), candidate strategies and per-run staging, an rmcp server with the nine editor tools, the `claude-code` adapter and runner, the reading pages behind Cloudflare Access, the CLI, and the Docker image. The digest is the unit of correctness, so the plan is sliced so that a complete path exists early (fixtures → items → candidates → staged → published, all in-process by the end of Phase 2) and the billed pieces (real `claude`, real mornings) come last.

## Architecture decisions (written at plan time, `docs/adr/`)

- **0001 Harness dispatch is an enum.** No `dyn Harness`, no `async-trait`; `HarnessKind` with one variant per adapter.
- **0002 One `Db` handle owns the connection and `spawn_blocking`.** Repo functions are sync over `&Connection`; pragmas WAL, `synchronous=NORMAL`, `busy_timeout=5000`, `foreign_keys=ON` on every open.
- **0003 Scheduler is `croner` + a sleep loop** with an injectable clock; no `tokio-cron-scheduler`.
- **0004 Coverage floors from one test run and two `llvm-cov report` calls** with complementary `--ignore-filename-regex`.
- **0005 `fastembed` on rustls features, lazy behind an `Embedder` trait**, fake embedder in tests.

Expected at `/ship` (`spec/r0.md` §11): 0006 rmcp, 0007 dom_smoothie, 0008 rusqlite, 0009 Access middleware in R0, 0010 seed deferred, 0011 Docker runtime with setup-token. Each is written by the task that makes the choice real.

## Dependency graph

```
config (schema, load, env)
   │
   ├── db: migrations const → ledger → Db handle → repo (sources, items, topics, runs, run_reads,
   │                                                      selections, digests, digest_items, reads,
   │                                                      editor_notes, run_lock, feed_issues)
   │        │
   │        ├── core::vector, core::time
   │        ├── core::embed (Embedder trait, fake, fastembed)
   │        ├── core::dedupe ─┐
   │        ├── core::extract ─┼── core::ingest ── commands::fetch
   │        ├── core::fetch ───┘        │
   │        │                            ▼
   │        ├── core::profile ── core::candidates ── core::staging ── core::digest
   │        │                                              │
   │        │                                              ▼
   │        │                                     mcp::{error, server, tools/*} ── commands::mcp
   │        │                                              │
   │        │                                              ▼
   │        │                                     editor::{digest_output, prompt, mcp_config} ── bin/gen-schemas
   │        │                                              │
   │        │                                              ▼
   │        │                harness::{types, claude_code, runner, verify, service_runner,
   │        │                          scheduler, scan_transcript} ── commands::{run, scan_transcript}
   │        │                                              │
   │        │                                              ▼
   │        └────────────────── web::{app, auth, routes/*, views/*} ── commands::serve
   │
   └── ops: commands::reembed, bin/check, deny.toml, Dockerfile, compose.yaml, bin/dc, CI
```

## Vertical slices

Each phase ends with a path that runs end to end at that layer, not a horizontal layer of the whole system:

| Phase | Slice that works at the end |
|---|---|
| 0 Foundation | `dailybrief migrate` opens or creates a database with the §5 schema; repo round-trips rows and vectors |
| 1 Ingest | `dailybrief fetch` turns `config/feeds.toml` into `items` rows with vectors, honouring 304s, caps and dedupe |
| 2 Select | In-process: fixtures → candidates → read → select ×30 → publish → `digests` rows, every violation refused |
| 3 MCP | An rmcp client drives all nine tools over stdio; `tools/list` is snapshotted |
| 4 Contract | `gen-schemas --check` is green; the prompt is byte-stable; `mcp.json` renders |
| 5 Harness | `dailybrief run` against the fake `claude` produces `runs`, `run_events`, a transcript, and retries once |
| 6 Web | `dailybrief serve` renders today's digest, logs a click, starts a run behind Access |
| 7 Ops | `bin/dc cargo test` is green in the container; CI runs `bin/check full`; the image runs the service |
| 8 Ship | First deployment, one real smoke run, one real digest run, rollback rehearsed, then three mornings |

## Task list

### Phase 0: Foundation (`config-db`)
- [x] Task 1: Crate skeleton and config loading
- [x] Task 2: Migration ledger and the `Db` handle; `migrate` verb
- [ ] Task 3: Vectors, time, and the repo for sources, items, topics

### Checkpoint A
- [ ] `cargo test` green; `cargo clippy --all-targets -- -D warnings` clean; `cargo fmt --check` clean
- [ ] `dailybrief migrate` on a temp path creates the schema; running it again applies nothing
- [ ] Review with human

### Phase 1: Ingest (`core-ingest`)
- [x] Task 4: `Embedder` trait, fake, fastembed implementation, cosine
- [x] Task 5: Canonical URL, hashes, item ids, duplicate detection
- [x] Task 6: HTTP client and feed fetch (conditional GET, caps, redirects)
- [x] Task 7: Article extraction with fixtures
- [x] Task 8: Ingest pipeline and the `fetch` verb

### Checkpoint B
- [x] Ingest integration test: wiremock feeds and pages → `items` rows with vectors; 304 path; failure counters; budget timeout persists partial results
- [x] Manual: `dailybrief fetch` on the host against `config/feeds.toml` writes items (36 feeds ok, 818 items, 2026-09-17)
- [x] Review with human (autonomous run)

### Phase 2: Select (`core-select`)
- [x] Task 9: Topic sync, profile vectors, `exploit`
- [x] Task 10: `cold_topic`, `popular_unmatched`, `search_items`
- [x] Task 11: Per-run reads and staging with every `select` rejection
- [x] Task 12: Digest validation and publish

### Checkpoint C
- [x] Scenario test: fixtures → `exploit` → `read_item` ×30 → `select` ×30 → `publish` → `digests` + `digest_items`; every §4 violation has a failing case
- [x] Review with human (autonomous run)

### Phase 3: MCP (`mcp-editor`)
- [x] Task 13: `ToolError`, server skeleton, `get_briefing`, the `mcp` verb
- [x] Task 14: `fetch_sources`, `report_feed_issue`, `editor_notes`
- [x] Task 15: `list_candidates`, `search_items`, `read_item`
- [x] Task 16: `select`, `publish_digest`, the fatal counter, the full violation matrix

### Checkpoint D
- [x] `tools/list` snapshot committed; nine names; input field names camelCase as in `SPEC.md` §4
- [x] `mcp` stdout purity test green; `ToolError` messages snapshotted
- [x] Review with human (autonomous run)

### Phase 4: Contract (`editor-contract`)
- [x] Task 17: `DigestOutput`, `gen-schemas` bin, prompt byte-stability, `mcp.json` rendering

### Checkpoint E
- [x] `cargo run --bin gen-schemas -- --check` green; diff of `schemas/digest.json` reviewed (same keys and types; bounds tightened to minimum 0; title added)

### Phase 5: Harness (`harness-claude`)
- [x] Task 18: Harness types, argv builder, env allowlist, stream-json events
- [x] Task 19: Spawning `claude`: stdout/stderr, wall clock, outcome mapping, fake `claude`
- [x] Task 20: Runner: lock, attempts, `runs` rows, transcript, verify; the `run` verb
- [x] Task 21: Scheduler loop and transcript scan; the `scan-transcript` verb

### Checkpoint F
- [x] `dailybrief run --attempts 2` with the fake `claude` on `PATH`: success, max-turns-then-success, killed, no-result each leave the right `runs` rows and files
- [x] Argv snapshot matches `SPEC.md` §3 flag for flag; env-scrub tests green
- [x] Review with human (autonomous run)

### Phase 6: Web (`web-read`)
- [x] Task 22: App shell, bind guard, security headers, digest pages and states; `serve` verb
- [x] Task 23: Click redirect, transcript download, `POST /run`, `/run/status`; scheduler wired into `serve`
- [x] Task 24: Cloudflare Access JWT middleware

### Checkpoint G
- [x] `oneshot` tests for every route and status code in `SPEC.md` §7b; 401 without a valid assertion
- [ ] Manual: page rendered at phone width in a real browser against a seeded temp DB (screenshot in the task note)
- [ ] Review with human

### Phase 7: Ops (`ops`)
- [x] Task 25: `reembed` verb, `bin/check`, `deny.toml`, cargo profiles, gitleaks
- [ ] Task 26: Dockerfile, `compose.yaml`, `bin/dc`, `.dockerignore`, compose test
- [ ] Task 27: CI workflow

### Checkpoint H
- [ ] `bin/dc cargo test` green inside the `test` container; `bin/check full` green; CI green on `main`
- [ ] `docker history` of the runtime image shows no secret; `.env` and `data/` excluded
- [ ] Review with human before spending subscription usage

### Phase 8: Ship (`/ship` prep)
- [ ] Task 28: First deployment, smoke run, real digest run, ship ADRs, rollback rehearsal

### Checkpoint: Complete
- [ ] `spec/r0.md` §9 criteria 1–7, 10, 11 met with evidence attached to the ship commit
- [ ] Criteria 8 and 9 (three mornings, phone read) tracked in the ship note until met

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| `schemas/digest.json` regenerated by schemars differs from the committed file (integer bounds, `format`, key order) | Med — the harness validates the final message against it | Task 17 compares parsed JSON, regenerates, and the diff is reviewed at Checkpoint E; acceptance is "same required fields, same types, `additionalProperties: false`", not byte equality |
| rmcp 3.4 API differs from the spec's sketch (`tool_router` macros, duplex transport, `structured_content`) | High — Phase 3 blocks | Task 13 starts by reading docs.rs for the resolved version and writing a 20-line spike test before any tool; fallback is hand-written `ServerHandler` impls without macros |
| Byte-vs-char slicing on non-ASCII article text panics at runtime | High — a morning run dies | Every cap test uses Vietnamese and emoji text (Tasks 5, 7, 11, 15); `chars().take(n)` is the only slicing form in `core::` |
| Two processes writing one SQLite file during a run (`serve` writes `run_events`, `mcp` writes `selections`) | Med — `SQLITE_BUSY` errors surface as tool failures | ADR 0002 pragmas; Task 20 has a test that writes from two `Db` handles on one file concurrently |
| `claude` child blocks on a full stderr pipe | Med — hang until the wall clock kills it | Task 19 drains stderr concurrently and has a chatty-stderr fake transcript |
| SIGTERM needs `nix`; a `libc` call would violate `forbid(unsafe_code)` | Low — caught at compile time | Task 19 adds `nix` with the `signal` feature; `lib.rs` has `#![forbid(unsafe_code)]` from Task 1 |
| fastembed/ort binary download or glibc mismatch in the container | Med — first `fetch` in Docker fails | Task 26 runs `dailybrief fetch` inside the image as its verification; bookworm-slim, never Alpine |
| Native Claude Code installer behaves differently from the npm build under `--restricted` | Med — smoke run fails | Task 26 verifies with a `claude --version` and `claude mcp list` inside the image; npm-on-node fallback documented in ADR 0011 |
| `--max-turns` semantics (counts tool round-trips inconsistently, `SPEC.md` §2) | Low — retry path is the norm | 120 already configured; the runner's retry-once is tested in Task 20 |
| Access JWT: JWKS fetch failure at startup | Low — every request 401s | Task 24 caches for 1 h, refetches on unknown `kid`, and serves 503 (not 401) while the cache is empty |
| Coverage floors unreachable in `web::` views | Low | Views are `maud` functions tested by rendering; ADR 0004 groups `web::` under the 60 % floor |

## Parallelization

- Safe in parallel after Checkpoint A: Tasks 4, 5, 6, 7 (independent core modules; Task 8 joins them).
- Safe in parallel after Checkpoint C: Task 17 with Tasks 13–16 (the contract type does not depend on the tools).
- Safe in parallel after Checkpoint F: Tasks 22–24 with Task 25.
- Sequential: Tasks 1–3, 9–12, 18–21, 26–28.

## Open questions (carried from `spec/r0.md` §13)

- Claude Code version to pin in the image: current at Task 26, recorded as a Dockerfile `ARG`.
- Where the existing `brief.db` copy comes from at first deployment: `docker cp` from the running data volume, documented in Task 28.
