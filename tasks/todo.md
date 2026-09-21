# M2 task list

Plan: `tasks/plan.md`. Spec: `spec/m2.md`. Every task: RED test → GREEN → `bin/dc bin/check task` → one commit. Deploys only between mornings.

---

## Phase 1: Trajectory (`trajectory`)

### Task 1: `harness::trajectory` — the `Event` enum, `parse_line`, the real-run fixture

**Description:** Add `src/harness/trajectory.rs` with an internally tagged `Event` enum (`system` → `SystemEvent { subtype, init: Option<InitEvent>, raw }`, `assistant` / `user` → `{ message: Message, timestamp }` where `Message` has `id`, `content: Vec<Block>` (`text`, `thinking`, `tool_use { id, name, input }`, `tool_result { tool_use_id, content }`, `#[serde(other)] Other`), `usage: Option<Usage>`; `tool_progress`, `rate_limit_event` keep their raw `Value`; `result` reuses `ResultEvent`; `#[serde(other)] Unknown`). `parse_line(&str) -> Event` never fails (invalid JSON → `Unknown`). `claude_code::parse_stream_line` and `event_type` become thin wrappers so the runner's stored `type` strings do not change. Copy the real transcript of run `2026-09-21-c8354589` verbatim to `tests/fixtures/transcripts/real-2026-09-21-c8354589.jsonl` after `scan-transcript` reports clean. Measure distinct assistant `message.id`s on it and write that number into ADR 0012's draft.

**Acceptance criteria:**
- [x] test: `every_fixture_line_parses_to_a_known_variant` (zero `Unknown` over the real fixture; counts per variant asserted: 107 assistant, 77 user, 148 system, 1 tool_progress, 3 rate_limit_event, 1 result).
- [x] test: `unknown_type_and_unknown_block_are_kept_not_dropped`; `new_system_subtype_keeps_its_name_and_invalid_json_is_unknown`; plus unit tests for string content, tool blocks, and a known type with a bad shape.
- [x] test: `event_type_strings_are_unchanged` (`system`, `assistant`, `user`, `result`, `rate_limit_event`, `tool_progress`, `unparseable` exactly as R0 stored them).
- [x] `docs/adr/0012-trajectory-model.md` drafted: 77 distinct ids vs `num_turns` 78 (the final round trip); per-line tokens do not sum to the result's, so Task 2 reconciles turns and wall clock only.

**Verification:**
- [x] `bin/dc cargo test trajectory`
- [x] `bin/dc bin/check task`
- [x] scan-transcript on the source run in the runtime image: clean

**Dependencies:** None
**Files likely touched:** `src/harness/trajectory.rs`, `src/harness/mod.rs`, `src/harness/claude_code.rs`, `tests/fixtures/transcripts/real-2026-09-21-c8354589.jsonl`, `docs/adr/0012-trajectory-model.md`
**Estimated scope:** Medium

### Task 2: `fold_turns`, `caps_used`, `retry_chain`; fixture reconciliation

**Description:** Pure functions over `&[RunEvent]` (parsing each payload with Task 1): `fold_turns` groups assistant lines by `message.id` into `Turn { n, at, tool: Option<String>, args_summary, result_chars, since_prev_secs, tokens: { input, output, cache_read, cache_create } }`, pairing each `tool_use.id` with its `tool_result`; `caps_used(&[Turn], &Caps, &ClaudeCodeSettings) -> CapsUsed` counts `read_item`, `select`, `WebSearch` calls, turns and wall clock (first to last timestamp, or `result.duration_ms`) against the caps; `retry_chain(&[RunRow]) -> Vec<AttemptOutcome>` for the run ids of one summary (attempt 1 → 2 by `kind` and `started_at` within the lock window). A `user` line without a matching `tool_use` id becomes a turn-less result line, never a panic.

**Acceptance criteria:**
- [x] test: `fixture_turns_reconcile_with_the_result_line` — turns 77 with `num_turns - 1 <= turns <= num_turns` (ADR 0012: `num_turns` 78 counts the final round trip); wall 500 s ± 1 of `duration_ms`; reads 32/45, selects 31/30 (hit), WebSearch 0/5, publish 1; 135 443 chars of read text. Per-turn tokens are not reconciled with the result (ADR 0012 measurement).
- [x] test: `turns_from_hand_written_events`; `orphan_tool_result_and_unknown_lines_do_not_panic`; `caps_used_marks_the_hit_cap`; `retry_chain_orders_attempts_and_ignores_unrelated_runs`.
- [ ] Coverage of `src/harness/trajectory.rs` ≥ 90 % lines (measured at Checkpoint C's `bin/check full`).

**Verification:**
- [x] `bin/dc cargo test trajectory`
- [x] `bin/dc bin/check task`

**Dependencies:** Task 1
**Files likely touched:** `src/harness/trajectory.rs`, `tests/it/trajectory.rs`, `tests/it/main.rs`, `docs/adr/0012-trajectory-model.md`
**Estimated scope:** Medium

### Task 3: Bounded-channel line delivery; lines never dropped under backpressure

**Description:** `HarnessKind::run` (and `ClaudeCodeAdapter::run`) take `tokio::sync::mpsc::Sender<(u64, String)>` instead of `impl FnMut(&str, u64)` and `await` each `send`; the runner creates `channel(1024)`, the writer task writes the file line first and inserts `run_events` second (Task-1-era behaviour kept: a DB error is logged, the file continues). A full channel logs once at warn per run. Update the fake-based tests mechanically (they collect through a channel now).

**Acceptance criteria:**
- [ ] test: `lines_are_never_dropped_under_backpressure` — fake knob `DAILYBRIEF_FAKE_LINES=10000` emits 10 000 numbered lines as fast as it can; a writer that sleeps 1 ms per line; every line is in the file and in `run_events`, in order, and the adapter returns `Success`.
- [ ] test: `argv_snapshot` unchanged; every existing harness/runner test green with the new signature.
- [ ] `CONSTRAINTS.md` check budget holds (`bin/check task` ≤ 4 min; the new test ≤ 15 s).

**Verification:**
- [ ] `bin/dc cargo test harness runner`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 2 (nothing functional; keeps the seam change after the model lands)
**Files likely touched:** `src/harness/types.rs`, `src/harness/claude_code.rs`, `src/harness/runner.rs`, `tests/fake-claude/claude`, `tests/it/harness.rs`, `tests/it/runner.rs`
**Estimated scope:** Medium

## Checkpoint A
- [ ] `bin/check task` green; every fixture line typed; turn count pinned to `result.num_turns` (or ADR 0012 explains)
- [ ] Review with human

---

## Phase 2: Run view (`run-view`)

### Task 4: `/runs/{id}` page (caps bar, retry chain, turn table); `/runs` columns; failed-state caps bar

**Description:** `src/web/routes/runs.rs` (`/runs` index moves here from `routes/log.rs`; new `show` for `/runs/{id}`) and `src/web/views/run.rs`: header with the caps bar (`reads 33/45 · selects 31/30 · web 8/5 · turns 78/120 · 8:20/15:00`, each hit cap marked), the retry chain (attempt 1 failed: reason → attempt 2 success), links to `/runs/{id}/log` and the transcript; one row per turn (n, time since previous, tool with short name, args summary ≤ 120 chars, result chars, tokens). `/runs` gains turns and wall-clock columns. The failed and running states on `/d/{date}` show the caps bar and link the run page. Page-specific CSS injected like the log page; the digest page stays under 15 KiB.

**Acceptance criteria:**
- [ ] test: `run_page_renders_caps_retry_and_every_turn` (insta snapshot of the turn table for the fixture; the caps bar text; a hit cap carries `class="hit"`).
- [ ] test: `run_page_for_a_running_run_reloads` (meta refresh) and `unknown_run_is_404`; `runs_index_shows_turns_and_wall`.
- [ ] test: `failed_state_shows_the_caps_bar_and_run_link`; `digest_page_size_cap_holds` (existing 15 KiB assertion) and `run_page_under_64_kib` for a 120-turn synthetic run.

**Verification:**
- [ ] `bin/dc cargo test web`
- [ ] `bin/dc bin/check task`
- [ ] Manual: phone-width screenshot of `/runs/{id}` on a seeded temp DB (task note)

**Dependencies:** Task 2
**Files likely touched:** `src/web/routes/runs.rs`, `src/web/routes/log.rs`, `src/web/routes/mod.rs`, `src/web/views/run.rs`, `src/web/views/state.rs`, `src/web/app.rs`, `tests/it/web.rs`
**Estimated scope:** Large (7 files, one feature) — split the failed-state change into Task 5 if the diff passes 400 lines

### Task 5: `dailybrief run --max-turns <n>`; forced-failure rendering against the fake

**Description:** `ServiceRunnerOptions.max_turns: Option<u32>` overrides `harness.claude-code.max_turns` for that run only (argv `--max-turns n`; the caps bar reads the override from the run's `usage_json` or the `result`'s `num_turns` at the hit). `dailybrief run --max-turns 5` plumbs it. Add `tests/fixtures/transcripts/max-turns-5-fake.jsonl` (five turns then `error_max_turns`, hand-written until Task 15 captures the real one) and prove the rendering end to end.

**Acceptance criteria:**
- [ ] test: `max_turns_override_reaches_argv` (snapshot variant); `run_verb_rejects_max_turns_zero`.
- [ ] test: `forced_failure_renders_turns_hit_on_home_and_run_page` — runner against the fake with the 5-turn fixture and `--attempts 1`, then `/` shows "No digest — run failed", the caps bar `turns 5/5` marked hit, and `/runs/{id}` lists five turns.

**Verification:**
- [ ] `bin/dc cargo test`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 4
**Files likely touched:** `src/harness/service_runner.rs`, `src/harness/runner.rs`, `src/harness/claude_code.rs`, `src/commands/run.rs`, `src/main.rs`, `tests/fixtures/transcripts/max-turns-5-fake.jsonl`, `tests/it/runner.rs`
**Estimated scope:** Medium

## Checkpoint B
- [ ] Phone-width check of `/runs/{id}` (screenshot); the fake forced failure shows `turns 5/5` on `/`
- [ ] Review with human

---

## Phase 3: Harness hardening (`harness-hardening`)

### Task 6: Stderr redaction before storage; `scan-transcript` covers `runs.error`; lossy stdout lines

**Description:** `claude_code::redact(tail, &[secrets]) -> String` replaces `sk-ant-[A-Za-z0-9_-]+` and the literal `CLAUDE_CODE_OAUTH_TOKEN` value (the adapter knows it) with `[redacted]` before the tail enters any `RunOutcome` message; `scan-transcript` gains `--run <id>` (or scans the `runs.error` of the transcript's run when given a path under `data/runs/`) and reports a token in `runs.error` as a finding. The stdout loop reads bytes per line and converts with `from_utf8_lossy`, so an invalid byte never ends the run.

**Acceptance criteria:**
- [ ] test: `stderr_tail_is_redacted_before_storage` (fake writes a fake token to stderr and exits 1; `runs.error` holds `[redacted]`, never the token).
- [ ] test: `scan_transcript_checks_runs_error` (a planted token in `runs.error` → finding, exit 1).
- [ ] test: `invalid_utf8_line_does_not_end_the_run` (fake emits `\xff` inside a line; the run still reaches its `result`; the stored line is lossy).

**Verification:**
- [ ] `bin/dc cargo test harness scan`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 3
**Files likely touched:** `src/harness/claude_code.rs`, `src/harness/scan_transcript.rs`, `src/commands/scan_transcript.rs`, `tests/fake-claude/claude`, `tests/it/harness.rs`
**Estimated scope:** Medium

### Task 7: SIGKILL-after-grace and killed-run transcript tests; fourth publish after `fatal` refused

**Description:** Fake knob `DAILYBRIEF_FAKE_IGNORE_TERM=1` (`trap '' TERM`); prove SIGKILL after `kill_grace` and that the pid is gone. Extend the killed-runs runner test to assert each killed attempt has its `run_events` rows, a transcript file with the lines before the kill, and `ended_at`. In `mcp::server`, after `publish_rejections == 3` every further `publish_digest` returns the same violation text plus `fatal: true` without touching the database (spec §11 #7).

**Acceptance criteria:**
- [ ] test: `run_hang_ignoring_sigterm_is_sigkilled_after_grace` (wall 300 ms, grace 500 ms; `Killed`; elapsed ≥ grace; `kill -0 <pid>` fails).
- [ ] test: `killed_runs_keep_their_transcript_and_events`.
- [ ] test: `publish_after_fatal_is_refused` (three empty publishes, then a valid 24 + 6 publish → refused, no `digests` row).

**Verification:**
- [ ] `bin/dc cargo test harness runner mcp`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 6
**Files likely touched:** `tests/fake-claude/claude`, `tests/it/harness.rs`, `tests/it/runner.rs`, `src/mcp/tools/publish_digest.rs`, `tests/it/mcp.rs`
**Estimated scope:** Medium

---

## Phase 4: Retention (`retention`)

### Task 8: `core::retention` and `dailybrief prune [--days] [--dry-run]` (ADR 0013)

**Description:** `retention::plan(conn, data_dir, now, days) -> Vec<PruneItem>` lists runs older than `days` (by `started_at`, never `status = running`), and `retention::apply(items, dry_run)` deletes in order: transcript file, run directory, `run_events` rows (one `repo::delete_run_events(run_id)`), never the `runs` row; returns counts. The verb prints the plan as JSON and, without `--dry-run`, applies it. `[retention] days = 60` in config (default 60, min 7).

**Acceptance criteria:**
- [ ] test: `prune_keeps_runs_rows_and_recent_runs` (three runs 30 / 61 / 400 days old on a temp data dir: two directories and their events gone, three rows stay, the 30-day run untouched).
- [ ] test: `prune_dry_run_deletes_nothing`; `prune_skips_running_runs`; `prune_tolerates_a_missing_directory` (rows still deleted).
- [ ] test: config `retention_days_below_seven_is_rejected`.

**Verification:**
- [ ] `bin/dc cargo test retention`
- [ ] `bin/dc bin/check task`
- [ ] `bin/dc cargo run -- prune --dry-run` on a temp data dir prints an empty plan

**Dependencies:** None
**Files likely touched:** `src/core/retention.rs`, `src/core/mod.rs`, `src/db/repo.rs`, `src/commands/prune.rs`, `src/main.rs`, `src/config/schema.rs`, `config/config.toml`, `docs/adr/0013-retention.md`
**Estimated scope:** Medium

### Task 9: Daily prune job in `serve` (`[retention]` config; 07:00 local)

**Description:** `[retention] cron = "0 7 * * *"` (validated like `[schedule].cron`); `serve` runs a second `Scheduler::run_loop` that calls `retention::plan` + `apply` and logs the counts. The job never overlaps a run (07:00 is after the 06:30 window and the wall clock; also skip if `status = running` exists, by construction of `plan`).

**Acceptance criteria:**
- [ ] test: `daily_prune_fires_after_the_morning_run` (injected clock stepping 06:30 → 07:00; the run job fires, then the prune job; order asserted through a shared log).
- [ ] test: `prune_job_error_does_not_stop_serve` (a failing apply logs and the loop continues).

**Verification:**
- [ ] `bin/dc cargo test scheduler serve`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 8
**Files likely touched:** `src/commands/serve.rs`, `src/harness/scheduler.rs`, `src/config/schema.rs`, `config/config.toml`, `tests/it/runner.rs`
**Estimated scope:** Small

## Checkpoint C
- [ ] `bin/check full` green; retention and harness tests pass; coverage not below R0 (95.4 % / 81.0 %)
- [ ] Review with human

---

## Phase 5: Web hardening (`web-hardening`)

### Task 10: `/run/status` from the lock holder; cap counts runs; JWKS refetch throttle; HS256 confusion test

**Description:** `GET /run/status` returns `{ active, heldBy? }` from `runner.lock_holder()` (or the in-process flag when no runner). `repo::count_runs_since` gains `attempt = 1`. `AccessVerifier` records the last forced refetch and answers 401 from the cached set if one happened under 60 s ago. Add the HS256-with-RSA-modulus test.

**Acceptance criteria:**
- [ ] test: `run_status_reflects_a_scheduled_run` (lock held by `scheduled@…` → `active: true`).
- [ ] test: `manual_run_cap_counts_runs_not_attempts` (two runs with two attempts each → the third manual run is accepted; the fourth is 429).
- [ ] test: `jwks_forced_refetch_is_throttled` (two unknown-kid tokens within a second → one JWKS request on the mock).
- [ ] test: `auth_rejects_hs256_token_with_rs_key_material` (401, no refetch).

**Verification:**
- [ ] `bin/dc cargo test web auth`
- [ ] `bin/dc bin/check task`

**Dependencies:** None
**Files likely touched:** `src/web/routes/run.rs`, `src/db/repo.rs`, `src/web/auth.rs`, `tests/it/web.rs`, `tests/it/auth.rs`
**Estimated scope:** Medium

---

## Phase 6: Core hardening (`core-hardening`)

### Task 11: `find_duplicate` on a per-batch `(id, vector)` projection; title-hash window; cosine near-duplicate ingest test

**Description:** `repo::list_item_vectors_since(since) -> Vec<(String, Vec<f32>)>` loaded once per `store_entries` batch and passed to `find_duplicate`; `has_title_hash` becomes `has_title_hash_since(hash, since)` with the 14-day window; the ingest test with an `Embedder` stub returning one unit vector for every text proves the cosine path.

**Acceptance criteria:**
- [ ] test: `find_duplicate_loads_vectors_once_per_batch` (a counting `Db` wrapper or a repo call count: one projection load for a batch of 20).
- [ ] test: `title_hash_dedupe_is_scoped_to_the_window` (same title 15 days apart is ingested twice; 5 days apart once).
- [ ] test: `ingest_skips_near_duplicate_by_cosine` (`new_items == 1` for two entries with different titles and URLs).

**Verification:**
- [ ] `bin/dc cargo test ingest dedupe repo`
- [ ] `bin/dc bin/check task`

**Dependencies:** None
**Files likely touched:** `src/db/repo.rs`, `src/core/dedupe.rs`, `src/core/ingest.rs`
**Estimated scope:** Medium

### Task 12: `fetch_sources` idempotent under concurrent calls; `verify` wording; extractor instrumentation (ADR 0014 measurement starts)

**Description:** A `tokio::sync::Mutex<Option<Value>>` held across the ingest makes a concurrent second `fetch_sources` wait and return the first report. `verify`'s failure reason says "final message did not parse as DigestOutput" (what is checked). `core::extract` returns which extractor produced the text; ingest logs `extractor = dom_smoothie | readability` per stored item and `fetch` reports the counts per run so the ADR 0014 measurement accrues from deployment.

**Acceptance criteria:**
- [ ] test: `fetch_sources_runs_once_under_concurrent_calls` (two simultaneous calls, one mock hit, identical reports).
- [ ] test: `verify_reason_names_the_parse` (the wording); `extract_reports_its_extractor` (long fixture → dom_smoothie; a fixture only readability handles → readability, or the fallback test from the review list).
- [ ] `docs/adr/0014-readability.md` drafted with the measurement method and the 2 % threshold.

**Verification:**
- [ ] `bin/dc cargo test mcp extract verify`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 11
**Files likely touched:** `src/mcp/server.rs`, `src/mcp/tools/fetch_sources.rs`, `src/harness/verify.rs`, `src/core/extract.rs`, `src/core/ingest.rs`, `docs/adr/0014-readability.md`
**Estimated scope:** Medium

## Checkpoint D
- [ ] `bin/check full` green; every `spec/m2.md` §9 #4 test exists except those of Tasks 13–15
- [ ] Deploy between mornings (afternoon); the next 06:30 run succeeds on the new image
- [ ] Review with human

---

## Phase 7: Ops and ship (`ops-m2`)

### Task 13: Vendored `install.sh` with sha256; `curl` and `zstd` purged from the runtime image

**Description:** `ops/claude-install.sh` is the installer downloaded on the day, with its sha256 in `ops/claude-install.sh.sha256` and in the Dockerfile; one `RUN` as root installs `curl zstd`, runs the vendored script as `app` for `CLAUDE_CODE_VERSION`, purges `curl zstd`, then `claude --version`. The CI image check gains `! command -v curl`.

**Acceptance criteria:**
- [ ] test (compose): `dockerfile_uses_the_vendored_installer_and_purges_download_tools` (no `claude.ai/install.sh` URL in the Dockerfile; `sha256sum -c`; `apt-get purge`).
- [ ] `docker run --rm dailybrief-rs:runtime sh -c 'command -v curl'` fails; `claude --version` prints 2.1.274.

**Verification:**
- [ ] `docker compose --profile app build` and the two image checks
- [ ] `bin/dc bin/check task`

**Dependencies:** Tasks 1–12 (last image change before ship)
**Files likely touched:** `Dockerfile`, `ops/claude-install.sh`, `ops/claude-install.sh.sha256`, `.github/workflows/ci.yml`, `tests/it/compose.rs`
**Estimated scope:** Small

### Task 14: `deny.toml` ↔ `CONSTRAINTS.md`; drop `harness.mcp_command`; tag `dailybrief-rs:r0`; `docs/runbook.md`; ADR 0015

**Description:** Add the three extra licences to `CONSTRAINTS.md` with reasons (or remove them from `deny.toml` if unused); remove `mcp_command`/`mcp_args` from config and schema (`current_exe()` stays); `docker tag dailybrief-rs:runtime dailybrief-rs:r0` on the box before the first M2 deploy and `DAILYBRIEF_IMAGE` in `compose.yaml` (default `dailybrief-rs:runtime`); `docs/runbook.md` with the token renewal date, advisory expiries (2026-12-17), prune, rollback by tag, the morning checklist; ADR 0015.

**Acceptance criteria:**
- [ ] test: `deny_licences_match_constraints` (parses both files); config `unknown key mcp_command is rejected` (deny_unknown_fields).
- [ ] `docker images` shows `dailybrief-rs:r0`; `docker compose --profile app config` shows the image variable resolved.
- [ ] `docs/runbook.md` and `docs/adr/0015-rollback-by-image-tag.md` exist.

**Verification:**
- [ ] `bin/dc bin/check full`

**Dependencies:** Task 13
**Files likely touched:** `deny.toml`, `CONSTRAINTS.md`, `config/config.toml`, `src/config/schema.rs`, `compose.yaml`, `docs/runbook.md`, `docs/adr/0015-rollback-by-image-tag.md`, `tests/it/compose.rs`
**Estimated scope:** Medium

### Task 15: Ship rehearsal — gate run, first prune, TypeScript container removal, ADRs final, CONSTRAINTS measured

**Description:** After a morning digest exists: `docker compose --profile app exec app dailybrief run --attempts 1 --max-turns 5` (pre-approved once); capture the transcript as `tests/fixtures/transcripts/max-turns-5.jsonl` and replace the hand-written fake fixture; check `/` and `/runs/{id}` on the phone. `prune --dry-run` on the box, show the list, then prune (pre-approved at 60 days). Ask before `docker rm dailybrief-app-1`. Finalise ADRs 0012–0014 with the measurements; update `CONSTRAINTS.md` measured; write the M2 ship note.

**Acceptance criteria:**
- [ ] `runs.status = failed`, `error_max_turns`; the caps bar shows `turns 5/5`; `scan-transcript` clean; fixture committed.
- [ ] First prune executed; `/data/runs` holds only the window; `runs` rows intact.
- [ ] `spec/m2.md` §9 criteria 1–6 evidenced in `docs/ship/m2.md`.

**Verification:**
- [ ] `bin/dc bin/check full`; CI green on the ship commit

**Dependencies:** Task 14
**Files likely touched:** `tests/fixtures/transcripts/max-turns-5.jsonl`, `docs/ship/m2.md`, `docs/adr/0012–0014`, `CONSTRAINTS.md`
**Estimated scope:** Medium (operations + docs)

## Checkpoint: Complete
- [ ] All `spec/m2.md` §9 criteria met with evidence
- [ ] `/review` (code-reviewer + security-auditor) and `/ship` run
