# M3 task list

Plan: `tasks/plan.md`. Spec: `spec/m3.md`. Every task: RED test → GREEN → `bin/dc bin/check task` → one commit. Deploys only between mornings, image tagged first; the migration deploy backs the database up first.

---

## Phase 1: Feedback DB (`feedback-db`)

### Task 1: Migration `0002_feedback` with rollback SQL and tests (ADR 0018)

**Description:** Append `Migration { id: "0002_feedback", sql }` to `MIGRATIONS`: `ratings(id INTEGER PRIMARY KEY AUTOINCREMENT, item_id TEXT NOT NULL REFERENCES items(id), digest_id TEXT REFERENCES digests(id), sign TEXT NOT NULL CHECK (sign IN ('up','down')), reason TEXT NOT NULL, at TEXT NOT NULL, UNIQUE(item_id))`, `proposals(id TEXT PRIMARY KEY, run_id TEXT REFERENCES runs(id), kind TEXT NOT NULL, payload_json TEXT NOT NULL, evidence_json TEXT NOT NULL, status TEXT NOT NULL CHECK (status IN ('pending','approved','rejected')), created_at TEXT NOT NULL, decided_at TEXT, applied_json TEXT)`, `ALTER TABLE runs ADD COLUMN role TEXT NOT NULL DEFAULT 'editor'`. A `ROLLBACK_0002` const (drop the two tables, rebuild `runs` without `role`, delete the ledger row) lives next to it, documented, never called by the service. Draft ADR 0018 with the DDL.

**Acceptance criteria:**
- [x] test: `migration_0002_applies_on_the_r0_schema_and_twice_is_a_noop`; `migration_0002_rollback_restores_the_0001_schema` (`sqlite_master` text-for-text on a temp DB); `unknown_applied_migration_ids_are_ignored`.
- [x] test: the R0 ledger tests pass with both ids (`applying_twice_is_a_noop`, the `migrate` verb test renamed `first_run_reports_every_migration_and_second_run_reports_nothing`, the `Db::open` test).

**Verification:**
- [x] `bin/dc cargo test migrate`
- [x] `bin/dc bin/check task`

**Dependencies:** None
**Files likely touched:** `src/db/migrations.rs`, `src/db/migrate.rs`, `docs/adr/0018-migration-0002-feedback.md`
**Estimated scope:** Small

### Task 2: `core::feedback` types, repo functions, transactional `apply`, writer-guard test (ADR 0016)

**Description:** `Sign`, `Reason` (the eight of `SPEC.md` §6 as an enum with stable snake_case strings), `ProposalStatus`, `ProposalChange` (five kinds, typed payloads, bounds in `TryFrom`), `Evidence { summary, rating_ids, read_count, feed_issue_ids, notes ≤ 500 }`. Repo: `upsert_rating` (replace by item), `delete_rating`, `list_ratings_since`, `insert_proposal`, `list_proposals(status)`, `get_proposal`, `decide_proposal`, plus the writers `set_topic_weight`, `set_topic_origin`, `insert_topic_from_proposal`, `set_source_enabled`, `insert_source_from_proposal`. `apply(conn, proposal_id, now)` runs in one transaction: loads the pending proposal, applies its change, records `applied_json`, sets `approved`; a missing target or a non-pending proposal returns a typed error and changes nothing. ADR 0016.

**Acceptance criteria:**
- [x] test: `reasons_round_trip_and_nothing_else_parses`; `proposal_change_bounds_are_refused` (weight 0 and 6, description 201 chars, unknown kind, empty source url).
- [x] test: `apply_changes_exactly_its_target_in_one_transaction` (one test per kind: row diffs); `apply_refuses_missing_target_and_non_pending` (no change, typed error); `ratings_upsert_replace_and_delete`.
- [x] test: `only_feedback_apply_writes_topics_and_sources` — greps `src/` for callers of the five writers; only `core::feedback::apply` (and `sync_topics` for `upsert_topic`, as today) may call them.

**Verification:**
- [x] `bin/dc cargo test feedback repo`
- [x] `bin/dc bin/check task`

**Dependencies:** Task 1
**Files likely touched:** `src/core/feedback.rs`, `src/core/mod.rs`, `src/db/repo.rs`, `tests/it/feedback.rs`, `tests/it/main.rs`, `docs/adr/0016-ratings-only-record.md`
**Estimated scope:** Medium

## Checkpoint A
- [x] `bin/check task` green; 0002 applies on a copy of `brief.db` and rolls back; the R0 ledger test still passes (rehearsed 2026-09-22 on a copy of the live file: 21 runs, integrity ok, apply → rollback → re-apply)
- [ ] Review with human

---

## Phase 2: Rating UI (`rating-ui`)

### Task 3: Rating buttons with inline reasons on the digest card; `POST /rate`; 24 KiB cap

**Description:** Each card gets 👍 and 👎 buttons (`hx-get="/rate/reasons?item=…&digest=…&sign=up"`, `hx-target` the card's rating slot); the partial lists the four reasons as buttons (`hx-post="/rate"` with hidden fields, `hx-swap="outerHTML"` of the slot); a rated card shows the choice and an "undo" (`hx-post="/rate"` with `sign=none`). Non-htmx fallback: plain forms. `POST /rate` checks same-origin like `/run`, validates item and digest ids exist, stores, replaces or deletes. The digest page cap in the test moves to 24 KiB (spec §11 #5).

**Acceptance criteria:**
- [x] test: `rate_stores_replaces_and_deletes` (three POSTs, `ratings` rows checked; other tables unchanged by row count); `rate_refuses_cross_origin_unknown_item_and_bad_reason` (403, 404, 400).
- [x] test: `digest_cards_carry_rating_controls_and_stay_under_24_kib`; `reasons_partial_lists_the_four_reasons_for_the_sign`; a rated card renders the choice and the undo.
- [x] No inline script: the page test asserts no `hx-on` and the CSP test is unchanged.

**Verification:**
- [x] `bin/dc cargo test web`
- [x] `bin/dc bin/check task`

**Dependencies:** Task 2
**Files likely touched:** `src/web/routes/rate.rs`, `src/web/routes/mod.rs`, `src/web/views/rate.rs`, `src/web/views/digest.rs`, `src/web/views/layout.rs`, `src/web/app.rs`, `tests/it/web.rs`
**Estimated scope:** Medium

### Task 4: Positive ratings join the exploit profile as read-like vectors

**Description:** `profile_vectors` adds the vectors of 👍-rated items within `candidate_days`, ranked with the reads by recency and weighted by the same `read_weight`; the `ProfileVector` source tag says `rating`. `get_briefing` is unchanged. A 👎 has no effect on scoring (ADR 0016).

**Acceptance criteria:**
- [x] test: `a_positive_rating_moves_an_items_rank_in_exploit` (seeded fixture: rate one item up, an item similar to it climbs, the list order otherwise stays); `a_negative_rating_changes_no_score`; `ratings_outside_the_window_are_ignored`.

**Verification:**
- [x] `bin/dc cargo test profile candidates`
- [x] `bin/dc bin/check task`

**Dependencies:** Task 2
**Files likely touched:** `src/core/profile.rs`, `src/db/repo.rs`, `src/core/candidates.rs`
**Estimated scope:** Small

## Checkpoint B
- [x] `bin/check task` green; the rating flow proven by test (phone check pending the Checkpoint E deploy)
- [ ] Review with human

---

## Phase 3: Curator tools (`curator-tools`)

### Task 5: `Role` in the run, `${RUN_ROLE}` in `mcp.json`, role-aware tool listing

**Description:** `Role { Editor, Curator }` (`runs.role`, `DAILYBRIEF_RUN_ROLE` env in the rendered `mcp.json`, read by the `mcp` verb; missing means editor). The server holds a `ToolRouter` composed for its role: editor = the nine R0 tools; curator = `get_feedback`, `get_profile`, `find_feeds`, `validate_feed`, `propose_change` (stubs that return a typed "not implemented" until Tasks 6–8). Spike first: two `#[tool_router(router = …)]` impls combined; fallback per the plan's risk table.

**Acceptance criteria:**
- [ ] test: `editor_listing_is_unchanged` (the R0 `tools_list` snapshot passes as is); `curator_listing_names_exactly_the_five_tools` (snapshot `tools_list_curator`); `an_editor_server_has_no_propose_change` (call → unknown tool).
- [ ] test: `mcp_json_carries_the_role` (rendered template); `mcp_verb_defaults_to_editor_without_the_env`.

**Verification:**
- [ ] `bin/dc cargo test mcp`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 2
**Files likely touched:** `src/mcp/server.rs`, `src/mcp/tools/mod.rs`, `src/commands/mcp.rs`, `src/config/load.rs`, `config/mcp.json`, `src/editor/mcp_config.rs`, `tests/it/mcp.rs`
**Estimated scope:** Medium

### Task 6: `get_feedback` and `get_profile`

**Description:** `core::curator_input`: `feedback(conn, now, window_days)` → ratings with item title/source/topic, reads, explore hit-rate (beyond-radar items shown / read / rated up over the window), feed issues; `profile(conn)` → topics (no descriptions) and sources with health. The two tools serialise them (camelCase, `spec/m3.md` §8).

**Acceptance criteria:**
- [ ] test: `get_feedback_window_and_shapes` (rmcp client; seeded ratings, reads, feed issues; 7-day window; field names); `get_profile_omits_topic_descriptions` (a planted description is absent, as `get_briefing` proves today).
- [ ] test: `explore_hit_rate_counts_beyond_radar_only`.

**Verification:**
- [ ] `bin/dc cargo test curator_input mcp`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 5
**Files likely touched:** `src/core/curator_input.rs`, `src/core/mod.rs`, `src/mcp/tools/get_feedback.rs`, `src/mcp/tools/get_profile.rs`, `src/db/repo.rs`, `tests/it/mcp.rs`
**Estimated scope:** Medium

### Task 7: `find_feeds` and `validate_feed`

**Description:** `core::feeds_discovery`: `find_feeds(http, url)` fetches the page through the guarded `Http`, collects `<link rel="alternate" type="application/(rss|atom)+xml">` hrefs (resolved against the page) and probes `/feed`, `/rss.xml`, `/atom.xml`, `/index.xml` with `validate_feed`; `validate_feed(http, url)` fetches with the body cap, parses with `feed-rs`, returns title, items per day over the last 30 days, `lastItemAt`, or a typed error. The two tools wrap them; the server remembers validated URLs for Task 8.

**Acceptance criteria:**
- [ ] test: `find_feeds_discovers_link_tags_and_common_paths` (wiremock page with two link tags, one probe hit); `validate_feed_reports_items_per_day` (RSS 2.0 and Atom fixtures with dates), `validate_feed_rejects_non_feeds_and_404` (typed error text).
- [ ] test: the private-address policy applies (a page linking to `http://10.0.0.1/feed` yields no feed and no request).

**Verification:**
- [ ] `bin/dc cargo test feeds_discovery mcp`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 5
**Files likely touched:** `src/core/feeds_discovery.rs`, `src/core/mod.rs`, `src/mcp/tools/find_feeds.rs`, `src/mcp/tools/validate_feed.rs`, `src/mcp/server.rs`, `tests/it/mcp.rs`
**Estimated scope:** Medium

### Task 8: `propose_change`: typed payloads, evidence, one row and nothing else

**Description:** Input `{ kind, payload, evidence }` parsed into `ProposalChange` and `Evidence`; `add_source` requires the URL to have passed `validate_feed` in this run (server memory from Task 7); `topic_weight` and `promote_explore_topic` require an existing topic; `disable_source` an existing enabled source. Writes one `proposals` row with `run_id`, returns `{ ok: true, proposalId }`; violations are typed rejections the prompt can quote.

**Acceptance criteria:**
- [ ] test: `propose_change_writes_one_row_and_nothing_else` (row counts of every table before and after); `propose_change_rejects_unknown_kind_bounds_and_unvalidated_source` (five rejection texts snapshotted); `propose_change_needs_an_existing_target`.

**Verification:**
- [ ] `bin/dc cargo test mcp`
- [ ] `bin/dc bin/check task`

**Dependencies:** Tasks 6, 7
**Files likely touched:** `src/mcp/tools/propose_change.rs`, `src/mcp/server.rs`, `src/mcp/error.rs`, `tests/it/mcp.rs`
**Estimated scope:** Medium

## Checkpoint C
- [ ] `bin/check task` green; editor `tools_list` snapshot unchanged; curator listing snapshotted
- [ ] Review with human

---

## Phase 4: Curator run (`curator-run`)

### Task 9: Curator prompt, `CuratorOutput` schema, runner role, `dailybrief curate`, fake fixture

**Description:** `prompts/curator.md` (byte-stable; inputs via tools; web search cap 8 from topic names; propose with evidence; end with the structured output); `CuratorOutput { runId, proposals: Vec<String>, notes ≤ 500 }` → `schemas/curator.json` (`gen-schemas --check` covers both files). The runner takes a `Role`: prompt, schema, user message and `mcp.json` role by role; `runs.role` written; verification for curator runs = the output parses and every listed proposal id exists (an empty list with notes is a success). `dailybrief curate [--attempts] [--max-turns]`. Fixture `curator-success.jsonl` hand-written (get_feedback → get_profile → propose_change ×2 → output).

**Acceptance criteria:**
- [ ] test: `curator_run_writes_proposals_under_role_curator` (runner + fake; two proposal rows from the transcript's tool calls, `runs.role = 'curator'`); `curator_run_with_no_proposals_is_a_success_with_notes`; `curator_prompt_is_byte_stable_and_names_no_date`; `gen_schemas_check_covers_curator_json`.
- [ ] test: `day_pages_ignore_curator_runs` (a failed curator run on a day with no digest renders "No run yet", not the failed state); `/runs` shows the role column.

**Verification:**
- [ ] `bin/dc cargo test runner web`; `bin/dc cargo run --bin gen-schemas -- --check`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 8
**Files likely touched:** `prompts/curator.md`, `schemas/curator.json`, `src/editor/curator_output.rs`, `src/bin/gen_schemas.rs`, `src/harness/runner.rs`, `src/harness/service_runner.rs`, `src/commands/curate.rs`, `src/main.rs`, `src/web/routes/digest.rs`, `tests/fixtures/transcripts/curator-success.jsonl`, `tests/it/runner.rs`
**Estimated scope:** Large (split the day-page change into Task 12 if the diff passes 400 lines)

### Task 10: `PreToolUse` hook policy, `dailybrief hook`, per-run `settings.json`, fake end-to-end (ADR 0019 draft)

**Description:** `harness::hook::decide(tool, input, state) -> Decision` (allow / refuse with reason): `mcp__dailybrief__propose_change` must parse as a `ProposalChange` with evidence; `WebSearch` counted in `<run dir>/hook-state.json`, refused from the ninth. `dailybrief hook pre-tool-use` reads Claude Code's hook JSON on stdin, exits 0 or 2 with the reason on stderr. The runner renders `<run dir>/settings.json` (`hooks.PreToolUse` matchers for the two tools, command = `<current_exe> hook pre-tool-use`) and appends `--settings <path>` to argv (snapshot updated). The fake gains `DAILYBRIEF_FAKE_RUN_HOOKS=1`: for each tool_use line it runs the hook from `settings.json` and emits a `hook` event line with the decision.

**Acceptance criteria:**
- [ ] test: `hook_allows_valid_proposals_and_refuses_unknown_kinds`; `hook_refuses_the_ninth_web_search_and_counts_per_run`; `hook_leaves_other_tools_alone`.
- [ ] test: `settings_json_is_rendered_per_run_and_argv_carries_it` (argv snapshot updated); `fake_run_with_hooks_records_refusals` (curator fixture with a bad proposal and nine searches: two `hook` refusals in the transcript).

**Verification:**
- [ ] `bin/dc cargo test hook harness`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 9
**Files likely touched:** `src/harness/hook.rs`, `src/harness/mod.rs`, `src/commands/hook.rs`, `src/main.rs`, `src/harness/claude_code.rs`, `src/harness/runner.rs`, `tests/fake-claude/claude`, `tests/it/harness.rs`, `docs/adr/0019-hooks-under-restricted.md`
**Estimated scope:** Medium

### Task 11: Scheduler as a job list; `[curator] cron`; Sunday 07:30 in `serve`

**Description:** `run_jobs(jobs: Vec<ScheduledJob>, clock, sleep, stop)` where `ScheduledJob { scheduler, name, job: Box<dyn FnMut() -> BoxFuture<Result<(), String>> + Send> }`; same catch-up rule; `run_two_loops` removed and its tests ported. `[curator] cron = "30 7 * * 0"` (validated); `serve` wires run, prune and curate (in that due order on a tie).

**Acceptance criteria:**
- [ ] test: the M2 scheduler tests pass on `run_jobs` unchanged in meaning; `curator_fires_on_sundays_only` (injected clock over eight days: one curator firing, seven runs, seven prunes); `ties_run_in_list_order`.
- [ ] test: config `curator_cron_is_validated`.

**Verification:**
- [ ] `bin/dc cargo test scheduler config`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 9
**Files likely touched:** `src/harness/scheduler.rs`, `src/commands/serve.rs`, `src/config/schema.rs`, `config/config.toml`
**Estimated scope:** Medium

## Checkpoint D
- [ ] `bin/check task` green; `dailybrief curate` against the fake writes proposals; the hook refuses in the fake run; Sundays only
- [ ] Review with human

---

## Phase 5: Approval (`approval-web`)

### Task 12: `/curator` page, approve/reject, `apply` as the only writer, the top-80 gate test

**Description:** `GET /curator`: pending proposals first (kind, payload rendered per kind, evidence as a definition list, the run link), then the last 30 decided; `POST /curator/{id}/approve` and `/reject` (same-origin, Access) call `core::feedback::apply` / `decide`; the page reloads (`HX-Refresh`). Every page header links `/curator`. The gate test: on the seeded fixture, approve a `topic_weight` proposal that doubles one topic's weight and assert the exploit top-80 differs by ≥ 5 ids.

**Acceptance criteria:**
- [ ] test: `curator_page_lists_pending_with_evidence_then_decided`; `approve_applies_and_reject_discards` (rows checked; a second approve is refused); `curator_posts_require_same_origin`.
- [ ] test: `approval_moves_the_exploit_top_80` (≥ 5 changed ids); `only_feedback_apply_writes_topics_and_sources` still passes with the new route.

**Verification:**
- [ ] `bin/dc cargo test web`
- [ ] `bin/dc bin/check task`

**Dependencies:** Task 11
**Files likely touched:** `src/web/routes/curator.rs`, `src/web/views/curator.rs`, `src/web/routes/mod.rs`, `src/web/views/mod.rs`, `src/web/views/layout.rs`, `src/web/app.rs`, `tests/it/web.rs`
**Estimated scope:** Medium

## Checkpoint E
- [ ] `bin/check full` green; coverage not below M2 (95.9 % / 83.2 %)
- [ ] Deploy between mornings: `docker tag` the running image, `cp brief.db brief.db.pre-0002` in the volume, rehearse the `m2` image on a copy carrying 0002, migrate live, `up -d`, watch the next 06:30 run
- [ ] Phone: rate one item; `/curator` opens (empty)
- [ ] Review with human

---

## Phase 6: Hardening (`hardening-m3`)

### Task 13: Security backlog

**Description:** Record the installed Claude Code 2.1.274 binary's sha256 in `ops/claude-2.1.274.sha256` and assert it after the install step; cap `/runs/{id}/log` at 2 000 events with a transcript link; an escaping regression test planting `<script>` in a tool input, a tool result and `runs.error`; NAT64 `64:ff9b::/96`, 6to4 `2002::/16`, `192.0.0.0/24`, `198.18.0.0/15` in the private list; `scan-transcript --run <id>`.

**Acceptance criteria:**
- [ ] test: `claude_binary_sha256_is_pinned` (compose test: the Dockerfile asserts the recorded hash); `log_page_caps_events`; `pages_escape_planted_html`; `ipv6_transition_ranges_are_private`; `scan_transcript_accepts_run_id`.

**Verification:**
- [ ] `docker compose --profile app build` (the hash assertion runs)
- [ ] `bin/dc bin/check task`

**Dependencies:** None
**Files likely touched:** `Dockerfile`, `ops/claude-2.1.274.sha256`, `src/web/views/log.rs`, `src/core/http.rs`, `src/commands/scan_transcript.rs`, `src/main.rs`, `tests/it/compose.rs`, `tests/it/web.rs`
**Estimated scope:** Medium

### Task 14: Harness/core backlog

**Description:** `selects` counts accepted selects only (a `select` whose result is `is_error` does not count); the `extracted` log fires after dedupe (stored items only); `routes::runs::show` folds once (`caps_of` takes `&[Turn]`); the stdout line buffer is capped at 4 MiB (an oversized line is stored truncated as `unparseable`); at `serve` start, `running` rows older than the lock window are marked `killed` with "orphaned at restart"; `page_with_css` gains a head slot and the meta refresh moves there.

**Acceptance criteria:**
- [ ] test: `selects_count_accepted_selects_only`; `extracted_log_fires_after_dedupe` (tracing subscriber capture); `run_page_folds_once` (a counting wrapper); `stdout_line_size_is_bounded`; `orphaned_running_rows_are_marked_at_start`; `meta_refresh_is_in_head`.

**Verification:**
- [ ] `bin/dc cargo test`
- [ ] `bin/dc bin/check task`

**Dependencies:** None
**Files likely touched:** `src/harness/trajectory.rs`, `src/core/ingest.rs`, `src/web/routes/runs.rs`, `src/harness/claude_code.rs`, `src/commands/serve.rs`, `src/db/repo.rs`, `src/web/views/layout.rs`, `src/web/views/run.rs`
**Estimated scope:** Medium

### Task 15: Deferred tests

**Description:** The test-engineer's remaining list: JWKS `forced_min_interval` injectable and `jwks_forced_refetch_resumes_after_the_interval`; `retry_chain_respects_window_and_kind`; `extract_falls_back_when_primary_is_too_short`; `caps_wall_from_timestamps`; `prune_and_title_window_boundaries`; `prune_days_below_seven_is_refused_by_the_cli`; `run_status_ignores_a_stale_holder`; `fetch_sources_concurrent_with_barrier`; heavier rows in `run_page_under_64_kib_for_120_turns`; drop the extra insert in `manual_run_cap_counts_runs_not_attempts`; fix the stale comment in `auth_refetches_on_unknown_kid_and_caches_otherwise`.

**Acceptance criteria:**
- [ ] Every named test exists and passes; no production code changes beyond a test constructor for the JWKS interval.

**Verification:**
- [ ] `bin/dc bin/check task`

**Dependencies:** None
**Files likely touched:** `tests/it/{auth,trajectory,web,runner,harness}.rs`, `src/core/{extract,retention,dedupe}.rs` tests, `src/mcp/tools/fetch_sources.rs` tests, `src/web/auth.rs` (test constructor)
**Estimated scope:** Medium

## Checkpoint F
- [ ] `bin/check full` green; every `spec/m3.md` §9 #7 test name exists and passes
- [ ] Review with human

---

## Phase 7: Ops and ship (`ops-m3`)

### Task 16: ADRs final, live migration follow-through, first Curator run and approval, ship

**Description:** ADR 0016 (ratings only record), 0017 (`seed` dropped, closing ADR 0010), 0018 (0002 DDL, rollback, the live apply's backup file name), 0019 (hooks result from the real run); ADR 0014 result from the extractor log counts (drop or keep `readability`, decided here); remove the TypeScript volumes (confirm first); runbook: weekly `/curator` review, `dailybrief curate`, migration backup and rollback, the Sunday schedule. One real `dailybrief curate` (confirm first) → proposals on `/curator` → approve one → the next morning's `list_candidates` result differs by ≥ 5 ids from the previous morning's. Record the first scheduled Sunday run. Close `docs/ship/r0.md` criterion 8 (three mornings). `CONSTRAINTS.md` measured. `docs/ship/m3.md`.

**Acceptance criteria:**
- [ ] The real curator run: `runs.role = 'curator'`, ≥ 1 proposal with evidence, `scan-transcript` clean, no other table changed (row counts); hook events observed or the fallback recorded in ADR 0019.
- [ ] First approval applied; the next-morning measure recorded; the first Sunday run recorded.
- [ ] `spec/m3.md` §9 criteria 1–8 evidenced in `docs/ship/m3.md`.

**Verification:**
- [ ] `bin/dc bin/check full`; CI green on the ship commit

**Dependencies:** Tasks 1–15
**Files likely touched:** `docs/adr/0014,0016–0019`, `docs/runbook.md`, `docs/ship/{r0,m2,m3}.md`, `CONSTRAINTS.md`
**Estimated scope:** Medium (operations + docs)

## Checkpoint: Complete
- [ ] `spec/m3.md` §9 criteria 1–8 met with evidence
- [ ] `/review` (code-reviewer + security-auditor) and `/ship` run
