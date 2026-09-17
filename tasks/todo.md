# R0 task list

Plan: `tasks/plan.md`. Spec: `spec/r0.md`. Bar: `CONSTRAINTS.md`. One commit per task, tests included. Verification commands are host `cargo` until Task 26 lands, then `bin/dc cargo …`. Every "test:" line in acceptance criteria names a test that must exist and pass.

---

## Phase 0: Foundation (`config-db`)

### Task 1: Crate skeleton and config loading

**Description:** Turn the hello-world crate into the package described in `spec/r0.md` §3/§5: `lib.rs` with `#![forbid(unsafe_code)]`, `main.rs` with a `clap` CLI that has no verbs yet beyond `--help`, and the `config` module that loads and validates `config/config.toml`, `feeds.toml`, `topics.toml` and the `DAILYBRIEF_*` env into typed structs via `TryFrom`, with the committed file's values as defaults.

**Acceptance criteria:**
- [x] `Config`, `Feed`, `Topic`, `Env` types exist; unknown keys in any TOML file are an error; ids must match `^[a-z0-9][a-z0-9_-]{0,63}$` and be unique; feed URLs must be http(s); the cron field is validated with `croner` (ADR 0003).
- [x] `feeds.toml` and `topics.toml` resolve next to `config.toml`; relative `data_dir` resolves against the current working directory; `DAILYBRIEF_CONFIG`, `DAILYBRIEF_DATA_DIR`, `DAILYBRIEF_BIND`, `DAILYBRIEF_RUN_ID`, `DAILYBRIEF_IN_CONTAINER` override.
- [x] test: `config::load::loads_committed_files` parses the real `config/` directory; `rejects_unknown_key`, `rejects_duplicate_id`, `rejects_non_http_url`, `rejects_bad_cron`; `defaults_match_committed_values` (an empty `[caps]` table yields the numbers in `config/config.toml`).

**Verification:**
- [x] `cargo test config::`
- [x] `cargo build --all-targets` with `RUSTFLAGS="-D warnings"`; `cargo clippy --all-targets -- -D warnings`; `cargo fmt --check`
- [x] `cargo run -- --help` lists no verbs yet and exits 0

**Dependencies:** None
**Files likely touched:** `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/config/mod.rs`, `src/config/schema.rs`, `src/config/load.rs`
**Estimated scope:** Medium

### Task 2: Migration ledger and the `Db` handle; `migrate` verb

**Description:** Implement ADR 0002: `db::Db` with `open`, `open_in_memory`, `call`, pragmas; `db::migrations` holding the `SPEC.md` §5 `0001_init` DDL as a const; `db::migrate` creating the `migrations` ledger with `CREATE TABLE IF NOT EXISTS` and applying pending ids in order, each in one transaction with an RFC 3339 UTC `applied_at`. Add the `migrate` CLI verb.

**Acceptance criteria:**
- [x] test: `db::migrate::applies_0001_on_empty_db` (all §5 tables and indexes exist); `applying_twice_is_a_noop` (second call returns an empty list, ledger row count unchanged); `ledger_row_has_rfc3339_millis_z`.
- [x] test: `db::pragmas_are_set` (WAL, `synchronous=1`, `busy_timeout=5000`, `foreign_keys=1` read back from a file-backed temp DB).
- [x] test: `db::call_maps_poisoned_lock_to_error`; `foreign_key_violation_is_rejected` (inserting an `items` row for a missing source fails).
- [x] `dailybrief migrate` prints the ids applied and exits 0; a second run prints nothing new.

**Verification:**
- [x] `cargo test db::`
- [x] `cargo run -- migrate` twice against `DAILYBRIEF_DATA_DIR=$(mktemp -d)`

**Dependencies:** Task 1
**Files likely touched:** `src/db/mod.rs`, `src/db/migrate.rs`, `src/db/migrations.rs`, `src/commands/mod.rs`, `src/commands/migrate.rs`
**Estimated scope:** Medium

### Task 3: Vectors, time, and the repo for sources, items, topics

**Description:** `core::vector` (384-d little-endian `f32` blob ↔ `Vec<f32>` via `chunks_exact(4)` / `to_le_bytes`, cosine), `core::time` (RFC 3339 UTC millis `Z` formatting, `days_ago`, local date in a zone), and the first third of `db::repo`: upsert/get/list for `sources`, `items`, `topics`, source health updates, `has_canonical_url`, `has_title_hash`, `list_items_since`, `set_topic_vector`, `delete_seed_topics_not_in`.

**Acceptance criteria:**
- [x] test: `core::vector::roundtrip_le_bytes`; `rejects_odd_length_blob`; `rejects_wrong_dimension`; `cosine_of_identical_is_one`, `cosine_of_orthogonal_is_zero`.
- [x] test: `core::time::formats_millis_z` (`2026-09-17T06:30:00.000Z`), `local_date_in_ho_chi_minh_crosses_midnight_utc`.
- [x] test: one repo test per function against `Db::open_in_memory()`, including `item_vector_null_when_absent` and `upsert_item_conflicts_on_canonical_url`.
- [x] test: `repo_is_the_only_sql_site` greps `src/` for `rusqlite::` outside `src/db/` and finds nothing.

**Verification:**
- [x] `cargo test core::vector core::time db::repo`
- [x] fast checks clean

**Dependencies:** Task 2
**Files likely touched:** `src/core/mod.rs`, `src/core/vector.rs`, `src/core/time.rs`, `src/db/repo.rs`
**Estimated scope:** Medium

## Checkpoint A
- [x] `cargo test` green; clippy `-D warnings` clean; `cargo fmt --check` clean
- [x] `dailybrief migrate` on a temp path creates the schema; second run applies nothing
- [x] Review with human before Phase 1

---

## Phase 1: Ingest (`core-ingest`)

### Task 4: `Embedder` trait, fake, fastembed implementation, cosine search helper

**Description:** Implement ADR 0005: the `Embedder` trait, `FakeEmbedder` (SHA-256 of the text → 384 floats → normalised), and `FastEmbedder` on the rustls features with lazy init and the model cache under `<data_dir>/models`. Add `embedder_for(&Config) -> Arc<dyn Embedder>`.

**Acceptance criteria:**
- [x] test: `core::embed::fake_is_deterministic_and_unit_length`; `fake_differs_for_different_text`; `empty_input_gives_empty_output`.
- [x] test (opt-in): `embed_real_bge_small_is_384d_unit_length`, runs only with `EMBED_REAL=1`, skipped with a printed reason otherwise (not `#[ignore]` without a string).
- [x] `FastEmbedder::new` does not load the model; the first `embed` does; `Cargo.toml` pins the feature set from ADR 0005 and `cargo tree -e features | grep native-tls` is empty.

**Verification:**
- [x] `cargo test core::embed`; `EMBED_REAL=1 cargo test embed_real` once on the host (evidence: output in the commit message)
- [x] fast checks clean

**Dependencies:** Task 3
**Files likely touched:** `Cargo.toml`, `src/core/embed.rs`
**Estimated scope:** Small

### Task 5: Canonical URL, hashes, item ids, duplicate detection

**Description:** `core::dedupe`: `canonical_url` (drop fragment, lowercase host, strip `utm_*`, `fbclid`, `gclid`, `mc_cid`, `mc_eid`, `ref`, `source`, `igshid`, `_hs*`; sort remaining query keys; strip one trailing slash on non-root paths), `title_hash` (lowercase, non-alphanumeric runs → one space, trim, SHA-256 hex), `content_hash` (whitespace-collapsed, SHA-256 hex), `item_id` (first 16 hex of SHA-256 of the canonical URL), and `find_duplicate` (URL → title → cosine > `dedupe_cosine` against items since 14 days).

**Acceptance criteria:**
- [x] test: table of ≥ 12 URL cases incl. tracking params, uppercase host, trailing slash, root path, fragment, unparsable input returned trimmed.
- [x] test: `title_hash_ignores_case_and_punctuation`; `title_hash_handles_vietnamese_diacritics` (`\p{L}` kept); `content_hash_ignores_whitespace_runs`.
- [x] test: `find_duplicate_by_url`, `_by_title`, `_by_cosine_over_threshold`, `_none_when_vector_missing`, `_ignores_items_older_than_window`.

**Verification:**
- [x] `cargo test core::dedupe`
- [x] fast checks clean

**Dependencies:** Task 3
**Files likely touched:** `Cargo.toml` (`sha2`, `url`), `src/core/dedupe.rs`
**Estimated scope:** Small

### Task 6: HTTP client and feed fetch (conditional GET, caps, redirects)

**Description:** `core::http` builds one `reqwest` client (rustls, gzip, brotli, `timeout = ingest.request_timeout_ms`, `redirect::Policy::limited(max_redirects)`, a `dailybrief/0.1` user agent) and a `read_capped` helper that streams a body up to `body_max_bytes` and errors past it. `core::fetch` fetches one feed with `If-None-Match` / `If-Modified-Since` from the `sources` row, parses it with `feed-rs`, and returns `FeedFetch::{NotModified, Entries(Vec<Entry>, etag, last_modified), Failed(String)}`; entries carry link, title, author, published, summary.

**Acceptance criteria:**
- [x] test (wiremock): `fetch_feed_parses_rss2_fixture`, `_parses_atom_fixture`, `_sends_conditional_headers`, `_returns_not_modified_on_304`, `_errors_past_body_cap` (a 3 MiB body), `_stops_after_max_redirects`, `_rejects_non_http_scheme`, `_times_out` (delayed response).
- [x] Fixtures `tests/fixtures/feeds/rss2.xml` and `atom.xml` are hand-written, ≤ 5 entries each, with one entry lacking `published`.

**Verification:**
- [x] `cargo test core::fetch core::http`
- [x] fast checks clean

**Dependencies:** Task 1
**Files likely touched:** `Cargo.toml` (`reqwest`, `feed-rs`, `wiremock`), `src/core/http.rs`, `src/core/fetch.rs`, `tests/fixtures/feeds/rss2.xml`, `tests/fixtures/feeds/atom.xml`
**Estimated scope:** Medium

### Task 7: Article extraction with fixtures

**Description:** `core::extract`: HTML → `Extracted { title, byline, text, word_count }` with `dom_smoothie`, falling back to `readability` when the first yields no body; `word_count` = `split_whitespace().count()`; a page with no extractable body returns `None` so ingest skips it. Three hand-written fixture pages.

**Acceptance criteria:**
- [x] test: `extracts_long_article_title_and_body` (fixture `long.html`, ≥ 800 words, nav/footer text absent from `text`); `extracts_short_article` (`short.html`); `returns_none_for_no_body` (`nobody.html`: a page of links only); `word_count_counts_unicode_words` (a Vietnamese paragraph).
- [x] test: `text_is_whitespace_normalised` (no runs of blank lines, no trailing spaces).

**Verification:**
- [x] `cargo test core::extract`
- [x] fast checks clean

**Dependencies:** Task 1
**Files likely touched:** `Cargo.toml` (`dom_smoothie`, `readability`), `src/core/extract.rs`, `tests/fixtures/html/long.html`, `tests/fixtures/html/short.html`, `tests/fixtures/html/nobody.html`
**Estimated scope:** Medium

### Task 8: Ingest pipeline and the `fetch` verb

**Description:** `core::ingest`: for every enabled feed, with `ingest.concurrency` in flight (`JoinSet` + `Semaphore`) under a `tokio::time::timeout` of `total_budget_ms`, fetch the feed, then for each new entry fetch the page, extract, dedupe, embed (`spawn_blocking`, batched per feed), and store the item; update `sources` (`etag`, `last_modified`, `failures`, `last_ok_at`, `last_error`). Return `IngestReport { fetched, new_items, per_feed: [{ source, status, new_items, error }] }`. Add the `fetch` verb.

**Acceptance criteria:**
- [x] test (wiremock + fake embedder + in-memory DB): `ingest_stores_new_items_with_vectors`; `ingest_skips_duplicates_by_url_and_title`; `ingest_304_counts_as_success_with_zero_new`; `ingest_failure_increments_failures_and_sets_last_error`; `ingest_success_resets_failures`; `ingest_persists_partial_results_on_budget_timeout` (one slow feed, budget 200 ms, the fast feed's items are stored and the report lists the slow one as failed); `ingest_skips_pages_with_no_body`.
- [x] `dailybrief fetch` prints the report as JSON on stdout and exits 0 even when some feeds failed; exits 1 only when config or DB fail.
- [x] Embedding is never called inside `Db::call` (a test wraps the fake embedder to assert it is not invoked while the DB mutex is held, or the code review confirms by structure — record which).

**Verification:**
- [x] `cargo test core::ingest`
- [x] Manual: `DAILYBRIEF_DATA_DIR=data cargo run -- fetch` on the host against `config/feeds.toml` (2026-09-17): 36 feeds ok, 0 failed, 818 new items, 2 min 5 s wall

**Dependencies:** Tasks 4, 5, 6, 7
**Files likely touched:** `src/core/ingest.rs`, `src/db/repo.rs`, `src/commands/fetch.rs`, `src/main.rs`
**Estimated scope:** Medium

## Checkpoint B
- [x] Ingest integration test green; `cargo test` green; fast checks clean
- [x] Manual host `fetch` produced items (evidence recorded)
- [x] Review with human before Phase 2 (autonomous run approved 2026-09-17; 86 tests green)

---

## Phase 2: Select (`core-select`)

### Task 9: Topic sync, profile vectors, `exploit`

**Description:** `core::profile`: `sync_topics` (upsert each topic from config by id, delete `seed` rows not in the file, embed `name + ". " + description` or `name` alone, store vectors), `profile_vectors` (topics weighted by `weight` × 0.5 when `saturation ≥ 3`; the 200 most recent reads with vectors weighted `0.5 / (1 + log10(rank))`), `exploit_score`. `core::candidates`: the shared pool (window, vector present, not shown) and the `exploit` strategy with `limit` clamped 1…100, snippet = first 200 chars whitespace-collapsed + `…`. Repo additions: `shown_item_ids(since)`, `list_read_vectors(limit)`.

**Acceptance criteria:**
- [x] test: `sync_topics_upserts_and_removes_stale_seed_rows`; `sync_topics_embeds_name_and_description`; `profile_weights_apply_saturation_damping`; `read_weight_decays_by_rank` (rank 1 = 0.5, rank 10 = 0.25); `exploit_score_is_zero_for_empty_profile`.
- [x] test: `exploit_excludes_shown_items`; `exploit_excludes_items_outside_window` (by `published_at`, and by `fetched_at` when null); `exploit_sorts_by_score_then_id`; `exploit_clamps_limit`; `snippet_cuts_on_chars_not_bytes` (Vietnamese text at the boundary).

**Verification:**
- [x] `cargo test core::profile core::candidates`
- [x] fast checks clean

**Dependencies:** Task 8
**Files likely touched:** `src/core/profile.rs`, `src/core/candidates.rs`, `src/db/repo.rs`
**Estimated scope:** Medium

### Task 10: `cold_topic`, `popular_unmatched`, `search_items`

**Description:** The other two strategies per `SPEC.md` §5: `cold_topic` (topics with a vector and `last_positive_at` null or older than 30 days; score = max cosine; exploit top-80 excluded), `popular_unmatched` (exploit score ≤ the 40th percentile, percentile = ascending sort, index `ceil(0.4·n) − 1` clamped; sort source weight desc, published/fetched desc, id asc; `note` on every row), and `search_items` (embed the query, cosine over the window including shown items, limit clamped 1…30).

**Acceptance criteria:**
- [x] test: `cold_topic_uses_only_cold_topics`; `cold_topic_excludes_exploit_top_80`; `popular_unmatched_percentile_formula` (table: n = 1, 5, 10 with expected cutoffs); `popular_unmatched_orders_by_source_weight_then_recency_then_id`; `popular_unmatched_carries_note`; `search_items_includes_shown_items`; `search_items_clamps_limit_to_30`.

**Verification:**
- [x] `cargo test core::candidates`
- [x] fast checks clean

**Dependencies:** Task 9
**Files likely touched:** `src/core/candidates.rs`
**Estimated scope:** Small

### Task 11: Per-run reads and staging with every `select` rejection

**Description:** `core::staging`: `read_item` (unknown id → error; the `caps.reads`-th distinct id is the last allowed, re-reads free; text = first `read_text_chars` chars; writes `run_reads`), `stage_selection` with the rejections in the exact `SPEC.md` §4 order and one-sentence messages naming the value and the cap, `section: none` deletes, re-select replaces and keeps position when the section is unchanged. Repo additions: `run_reads`, `selections` CRUD. `StagingError` is a `thiserror` enum with one variant per rejection.

**Acceptance criteria:**
- [x] test: `read_item_caps_distinct_ids_at_45`; `read_item_reread_is_free`; `read_item_truncates_by_chars` (5 001-char Vietnamese text → `truncated: true`, `text.chars().count() == 5000`); `read_item_unknown_id`.
- [x] test: one test per rejection, in order: `select_rejects_unknown_item`, `_unread_item`, `_empty_summary`, `_empty_why`, `_summary_over_80_words`, `_why_over_25_words`, `_unknown_topic_lists_known_names`, `_beyond_radar_without_reason`, `_beyond_radar_bad_reason`, `_shown_item`, `_fifth_from_same_source`, `_ninth_in_same_topic`, `_twenty_fifth_for_you`, `_seventh_beyond_radar`; plus `select_none_unstages`, `select_replaces_keeping_position`, `select_topic_match_is_case_insensitive`.
- [x] test: `insta` snapshot of every `StagingError` message.

**Verification:**
- [x] `cargo test core::staging`
- [x] fast checks clean

**Dependencies:** Task 10
**Files likely touched:** `src/core/staging.rs`, `src/db/repo.rs`
**Estimated scope:** Medium

### Task 12: Digest validation and publish

**Description:** `core::digest`: `validate_staged` returning every violation sentence from `SPEC.md` §4 (`publish_digest`), and `publish` (already-published check; `digest_id = <local date>-<run id>`; `digests` + `digest_items` in one transaction with positions renumbered 1… per section in staged order). Repo additions: `digests`, `digest_items`, `get_digest_by_run`, `insert_digest`.

**Acceptance criteria:**
- [x] test: `publish_rejects_wrong_counts` (23 + 6, 24 + 7), `_unread_staged_item`, `_shown_item`, `_beyond_radar_without_reason`, `_source_over_cap`, `_topic_over_cap`, `_already_published`; `publish_writes_atomically_and_renumbers_positions`; `publish_id_uses_local_date` (a run at 23:30 UTC gets the next day's date in Ho Chi Minh).
- [x] test (scenario, Checkpoint C): `full_run_in_process` — fixtures → `exploit` → 30 reads → 30 selects → publish → 30 `digest_items`.

**Verification:**
- [x] `cargo test core::digest`
- [x] fast checks clean

**Dependencies:** Task 11
**Files likely touched:** `src/core/digest.rs`, `src/db/repo.rs`, `tests/it/main.rs`, `tests/it/scenario.rs`
**Estimated scope:** Medium

## Checkpoint C
- [x] `full_run_in_process` green; every §4 violation has a failing case; `cargo test` green
- [x] Review with human before Phase 3 (autonomous run approved 2026-09-17; 125 tests green)

---

## Phase 3: MCP (`mcp-editor`)

### Task 13: `ToolError`, server skeleton, `get_briefing`, the `mcp` verb

**Description:** Read docs.rs for rmcp 3.4 and write a spike test first (a server with one tool driven by an rmcp client over `tokio::io::duplex`). Then `mcp::error::ToolError` (variants wrapping `StagingError`, `DigestViolations`, `UnknownFeed`, `NotesTooLong`, `Internal`) with a conversion into a `CallToolResult` that carries `content` text + `structured_content` and `is_error: true`; `mcp::server` with `#[tool_router]` and the `get_briefing` tool (`SPEC.md` §4 shape, topic descriptions never included); `commands::mcp` reading `DAILYBRIEF_RUN_ID`, logging to stderr only, serving stdio.

**Acceptance criteria:**
- [x] test: `mcp::tools_list_has_get_briefing_with_camel_case_fields` via the in-process client; `get_briefing_returns_shape` (keys `date, timezone, profile.topics[].name/weight, shownIds, feedHealth, notes, caps`); `get_briefing_omits_topic_descriptions`.
- [x] test: `tool_error_becomes_is_error_result_not_protocol_error`; `internal_panic_becomes_one_sentence`.
- [x] test: `mcp_stdout_is_json_rpc_only` — spawn `dailybrief mcp` with `RUST_LOG=trace`, send `initialize` + `tools/list`, assert every stdout line parses as JSON-RPC.
- [x] Manual: `claude mcp list` is **not** used (it would spend nothing but is not needed); the in-process client is the proof.

**Verification:**
- [x] `cargo test mcp::`
- [x] fast checks clean

**Dependencies:** Task 12
**Files likely touched:** `Cargo.toml` (`rmcp`), `src/mcp/mod.rs`, `src/mcp/error.rs`, `src/mcp/server.rs`, `src/mcp/tools/get_briefing.rs`, `src/commands/mcp.rs`
**Estimated scope:** Medium

### Task 14: `fetch_sources`, `report_feed_issue`, `editor_notes`

**Description:** Three tools over existing core: `fetch_sources` (runs ingest; idempotent within the process via an `AtomicBool`, second call returns the first report), `report_feed_issue` (`kind ∈ {dead, paywalled, junk, duplicate}`; writes `feed_issues` and `sources.last_error`; unknown feed → error), `editor_notes` (`op ∈ {read, replace}`; ≤ 2 000 chars; single row).

**Acceptance criteria:**
- [x] test: `fetch_sources_is_idempotent_within_run`; `report_feed_issue_unknown_feed_is_error`; `report_feed_issue_writes_row_and_last_error`; `editor_notes_roundtrip`; `editor_notes_rejects_over_2000_chars` (2 001 chars of Vietnamese counts as 2 001, not bytes).
- [x] `tools/list` now has four tools; the snapshot is updated.

**Verification:**
- [x] `cargo test mcp::`
- [x] fast checks clean

**Dependencies:** Task 13
**Files likely touched:** `src/mcp/server.rs`, `src/mcp/tools/fetch_sources.rs`, `src/mcp/tools/report_feed_issue.rs`, `src/mcp/tools/editor_notes.rs`, `src/db/repo.rs`
**Estimated scope:** Medium

### Task 15: `list_candidates`, `search_items`, `read_item`

**Description:** Three tools over `core::candidates` and `core::staging`: `list_candidates { strategy, limit }` → `{ candidates: [...] }`, `search_items { query, limit }` (embed in `spawn_blocking`), `read_item { id }` → `{ id, title, source, text, wordCount, truncated }`.

**Acceptance criteria:**
- [x] test: `list_candidates_rejects_unknown_strategy_at_schema_level` (enum in the input schema); `list_candidates_exploit_shape`; `search_items_embeds_query_locally`; `read_item_cap_error_message_matches_snapshot`; `read_item_text_is_5000_chars_max`.
- [x] `tools/list` has seven tools; snapshot updated.

**Verification:**
- [x] `cargo test mcp::`
- [x] fast checks clean

**Dependencies:** Task 14
**Files likely touched:** `src/mcp/server.rs`, `src/mcp/tools/list_candidates.rs`, `src/mcp/tools/search_items.rs`, `src/mcp/tools/read_item.rs`
**Estimated scope:** Medium

### Task 16: `select`, `publish_digest`, the fatal counter, the full violation matrix

**Description:** `select` (input enum for `section` incl. `none`; `reason` optional; result `{ ok, replaced, staged }`), `publish_digest` (`{ ok: true, digestId, date, forYou, beyondRadar }` or `{ ok: false, violations, fatal? }` with `is_error: true`; `fatal: true` on the third rejection in this process via an `AtomicU8`). The integration test drives the whole nine-tool trajectory through the client.

**Acceptance criteria:**
- [x] test: `select_every_rejection_through_mcp` (one sub-case per Task 11 rejection, asserting `isError` and the exact sentence); `publish_third_rejection_is_fatal`; `publish_success_shape`; `full_trajectory_through_mcp` (get_briefing → list_candidates ×3 → read_item ×30 → select ×30 → publish_digest).
- [x] test: `tools_list_snapshot` — nine names, sorted, with input schemas, via `insta`.
- [x] `ToolError` messages snapshot covers every variant.

**Verification:**
- [x] `cargo test mcp::`
- [x] fast checks clean

**Dependencies:** Task 15
**Files likely touched:** `src/mcp/server.rs`, `src/mcp/tools/select.rs`, `src/mcp/tools/publish_digest.rs`, `tests/it/mcp.rs`
**Estimated scope:** Medium

## Checkpoint D
- [x] Nine tools proven; `tools/list` snapshot committed; stdout purity green; `cargo test` green
- [x] Review with human before Phase 4 (autonomous run approved 2026-09-17; 145 tests green)

---

## Phase 4: Contract (`editor-contract`)

### Task 17: `DigestOutput`, `gen-schemas` bin, prompt byte-stability, `mcp.json` rendering

**Description:** `editor::digest_output::DigestOutput` (`digestId`, `date` with the `^\d{4}-\d{2}-\d{2}$` pattern, `forYou`, `beyondRadar`, optional `notes ≤ 300`), `deny_unknown_fields`; `src/bin/gen_schemas.rs` emitting draft-07 and comparing as parsed JSON with `--check`; `editor::prompt` test that `prompts/editor.md` contains no date and no UUID; `editor::mcp_config::render` substituting `${MCP_COMMAND}` (= `std::env::current_exe()`), `${MCP_ARGS}`, `${RUN_ID}`, `${CONFIG_PATH}`, `${DATA_DIR}` and dropping `_comment`.

**Acceptance criteria:**
- [x] test: `digest_output_rejects_unknown_fields`; `digest_output_date_pattern`; `schema_is_draft07_with_additional_properties_false`.
- [x] test: `editor_prompt_is_byte_stable` (no `\d{4}-\d{2}-\d{2}`, no UUID-shaped string); `smoke_prompt_too`.
- [x] test: `render_mcp_config_substitutes_all_placeholders_and_drops_comment`; `rendered_config_is_valid_json_with_args_array`.
- [x] `cargo run --bin gen-schemas -- --check` exits 0 after regeneration; the diff of `schemas/digest.json` against the committed input is attached to the commit and reviewed at Checkpoint E (same required keys, same types, `additionalProperties: false`).

**Verification:**
- [x] `cargo test editor::`; `cargo run --bin gen-schemas -- --check`
- [x] fast checks clean

**Dependencies:** Task 12 (independent of Tasks 13–16)
**Files likely touched:** `src/editor/mod.rs`, `src/editor/digest_output.rs`, `src/editor/prompt.rs`, `src/editor/mcp_config.rs`, `src/bin/gen_schemas.rs`, `schemas/digest.json`
**Estimated scope:** Medium

## Checkpoint E
- [x] `gen-schemas --check` green; schema diff reviewed (autonomous run 2026-09-17): same required keys, same property types, additionalProperties false; integers now minimum 0 with format uint32 instead of ±2^53; a `title` was added; `notes` stays a plain string (the null alternative schemars adds for Option is stripped)
- [x] Review with human before Phase 5 (autonomous run; 152 tests green)

---

## Phase 5: Harness (`harness-claude`)

### Task 18: Harness types, argv builder, env allowlist, stream-json events

**Description:** Implement ADR 0001's types: `HarnessRequest`, `RunOutcome::{Success, Failed { reason, message, exit_code, result, init }, Killed}`, `HarnessKind`. `harness::claude_code::build_argv` (the exact `SPEC.md` §3 flag list), `build_env` (allowlist + `DAILYBRIEF_*`; refuse `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN`; require `CLAUDE_CODE_OAUTH_TOKEN`), and the `StreamEvent` internally-tagged enum (`system/init`, `result`, other with raw `Value`, `Unparseable`).

**Acceptance criteria:**
- [ ] test: `argv_snapshot` (`insta`, flag for flag against `SPEC.md` §3); `argv_never_contains_bare`.
- [ ] test: `env_refuses_anthropic_api_key`, `_refuses_auth_token`, `_requires_oauth_token`, `_passes_only_allowlist_and_dailybrief_prefix`, `_drops_everything_else`.
- [ ] test: `parses_init_event`, `parses_result_success`, `parses_result_error_max_turns`, `passes_through_unknown_type` (`rate_limit_event`), `unparseable_line_is_kept_raw` — on hand-written minimal fixture lines under `tests/fixtures/transcripts/`.

**Verification:**
- [ ] `cargo test harness::`
- [ ] fast checks clean

**Dependencies:** Task 17
**Files likely touched:** `src/harness/mod.rs`, `src/harness/types.rs`, `src/harness/claude_code.rs`, `tests/fixtures/transcripts/success.jsonl`, `tests/fixtures/transcripts/max-turns.jsonl`
**Estimated scope:** Medium

### Task 19: Spawning `claude`: stdout/stderr, wall clock, outcome mapping, fake `claude`

**Description:** `ClaudeCodeAdapter::run`: `tokio::process::Command` with `env_clear()` + the built env, `kill_on_drop(true)`, cwd = run dir, stdin closed; stdout read line by line and handed to `hooks.on_line(raw, seq)` while stderr is drained concurrently (last 4 kB kept); wall clock via `tokio::time::timeout` → SIGTERM with `nix::sys::signal::kill`, SIGKILL after 10 s; outcome mapping per `SPEC.md` §3. `tests/fake-claude/claude` replays a transcript file named by `FAKE_CLAUDE_TRANSCRIPT`, honours `FAKE_CLAUDE_HANG=1`, `FAKE_CLAUDE_EXIT=<n>`, `FAKE_CLAUDE_STDERR_BYTES=<n>`.

**Acceptance criteria:**
- [ ] test: `run_success_transcript`; `run_error_max_turns`; `run_hang_is_killed_by_wall_clock` (wall clock 300 ms, SIGTERM observed by the fake, `Killed`); `run_nonzero_exit_without_result`; `run_ends_without_result_line`; `run_survives_chatty_stderr` (1 MiB on stderr, still completes); `run_delivers_every_stdout_line_in_order`.
- [ ] No `unsafe`; `nix` is the only signal path (grep test for `libc::`).

**Verification:**
- [ ] `cargo test harness::claude_code`
- [ ] fast checks clean

**Dependencies:** Task 18
**Files likely touched:** `Cargo.toml` (`nix`), `src/harness/claude_code.rs`, `tests/fake-claude/claude`, `tests/fixtures/transcripts/no-result.jsonl`, `tests/it/harness.rs`
**Estimated scope:** Medium

### Task 20: Runner: lock, attempts, `runs` rows, transcript, verify; the `run` verb

**Description:** `harness::runner`: acquire `run_lock` (single row, TTL `2 × wall_clock + 5` min, stale takeover), then up to `max_attempts` attempts, each with a fresh run id (`YYYY-MM-DD-<8 hex>`), run dir, rendered `mcp.json`, empty transcript, `runs` row (`running` → final), every line appended to the transcript file and `run_events`, outcome classified with `harness::verify` (structured output validates against `DigestOutput` **and** a digest row exists for the run **and** ids match). `service_runner` syncs topics before running. `commands::run` with `--harness --kind --attempts --no-verify --prompt --schema --message`. Repo additions: `runs`, `run_events`, `run_lock`.

**Acceptance criteria:**
- [ ] test: `runner_retries_once_then_succeeds` (max-turns then success → two `runs` rows, second `success`); `runner_marks_killed`; `runner_marks_failed_when_no_digest_row` (harness success, no digest → `failed` with the verify message); `runner_marks_failed_on_digest_id_mismatch`; `runner_writes_transcript_and_run_events_in_order`; `runner_refuses_when_locked`; `runner_takes_over_stale_lock`; `runner_releases_lock_on_adapter_panic`.
- [ ] test: `two_db_handles_write_one_file_concurrently` (a `serve`-side writer and an `mcp`-side writer interleave 200 writes without `SQLITE_BUSY`).
- [ ] `dailybrief run` with the fake `claude` on `PATH` prints the `RunSummary` JSON and exits 0/1/3 (success / failed / locked).

**Verification:**
- [ ] `cargo test harness::runner harness::verify`
- [ ] `PATH=tests/fake-claude:$PATH FAKE_CLAUDE_TRANSCRIPT=... cargo run -- run --attempts 1 --no-verify` on a temp data dir
- [ ] fast checks clean

**Dependencies:** Task 19
**Files likely touched:** `src/harness/runner.rs`, `src/harness/verify.rs`, `src/harness/service_runner.rs`, `src/commands/run.rs`, `src/db/repo.rs`
**Estimated scope:** Medium

### Task 21: Scheduler loop and transcript scan; the `scan-transcript` verb

**Description:** Implement ADR 0003 (`harness::scheduler` with injectable clock and sleep) and `harness::scan_transcript` (forbidden fragments: vault paths, `sk-ant-`, `OAUTH_TOKEN=`; any topic description ≥ 12 chars; every `read_item` `tool_result` must be a prefix of the stored item text for a known id; more than 5 `WebSearch` tool uses is a finding). `commands::scan_transcript` exits 1 on findings.

**Acceptance criteria:**
- [ ] test: `scheduler_next_occurrence_in_ho_chi_minh` (06:30 local = 23:30 UTC previous day); `scheduler_fires_once_per_tick_with_fake_clock`; `scheduler_skips_when_lock_held_and_logs`.
- [ ] test: `scan_flags_vault_path`, `_flags_topic_description`, `_flags_foreign_read_item_text`, `_flags_unknown_item_id`, `_flags_more_than_5_web_searches`, `_passes_clean_transcript` — on hand-written fixtures.

**Verification:**
- [ ] `cargo test harness::scheduler harness::scan_transcript`
- [ ] fast checks clean

**Dependencies:** Task 20
**Files likely touched:** `Cargo.toml` (`croner`), `src/harness/scheduler.rs`, `src/harness/scan_transcript.rs`, `src/commands/scan_transcript.rs`, `tests/fixtures/transcripts/leaky.jsonl`
**Estimated scope:** Medium

## Checkpoint F
- [ ] Runner end-to-end with the fake `claude` in all outcome shapes; argv and env tests green; `cargo test` green
- [ ] Review with human before Phase 6

---

## Phase 6: Web (`web-read`)

### Task 22: App shell, bind guard, security headers, digest pages and states; `serve` verb

**Description:** `web::app` (`axum` router with `AppState { db, config, runner: Option<…> }`, a layer adding `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`, 404 fallback), `assert_bind_allowed`, `web::views::{layout, digest, state}` in `maud` (mobile-first, system fonts, htmx script tag), `routes::digest` for `/` and `/d/{date}` with the four states (published / failed with error + transcript link + Refresh / running / none). `commands::serve` serving on `bind:port` with graceful shutdown on SIGTERM.

**Acceptance criteria:**
- [ ] test (`oneshot`): `root_redirects_or_renders_today`; `day_renders_published_digest_with_24_then_6`; `beyond_radar_cards_show_reason_badge`; `day_renders_failed_state_with_transcript_link`; `day_renders_running_state`; `day_renders_no_run_state`; `unknown_route_is_404`; `security_headers_on_every_response`; `html_under_15_kb_for_30_items`.
- [ ] test: `bind_guard_allows_loopback`, `_allows_any_in_container`, `_refuses_otherwise`.

**Verification:**
- [ ] `cargo test web::`
- [ ] `cargo run -- serve` on a temp data dir; `curl -s localhost:8788/ | head`
- [ ] fast checks clean

**Dependencies:** Task 21
**Files likely touched:** `Cargo.toml` (`axum`, `tower-http`, `maud`), `src/web/mod.rs`, `src/web/app.rs`, `src/web/views/*.rs`, `src/web/routes/digest.rs`, `src/commands/serve.rs`
**Estimated scope:** Large — split views into their own commit if it grows past five files

### Task 23: Click redirect, transcript download, `POST /run`, `/run/status`; scheduler wired into `serve`

**Description:** `routes::redirect` (`/r/{id}`: insert `reads(item, digest shown in, now)`, 302 to `canonical_url`, 404 unknown), `routes::transcript` (`/runs/{id}/transcript` streams the file), `routes::run` (`POST /run`: 503 when no runner, 403 when cross-origin per `SPEC.md` §7b, 429 past 3 manual runs in 24 h, 409 when a run is active, else spawn and 202 or 303 for browsers; `GET /run/status`). `serve` starts the scheduler task. Repo additions: `reads`, `count_runs_since`.

**Acceptance criteria:**
- [ ] test: `redirect_logs_read_and_302s`; `redirect_unknown_is_404`; `transcript_streams_file`; `transcript_unknown_is_404`; `post_run_503_without_runner`; `post_run_403_cross_origin` (`Origin` host mismatch; `Sec-Fetch-Site: cross-site`); `post_run_passes_same_origin_and_no_origin`; `post_run_429_after_cap`; `post_run_409_when_active`; `post_run_202_json_and_303_html`; `run_status_reflects_active`.

**Verification:**
- [ ] `cargo test web::`
- [ ] fast checks clean

**Dependencies:** Task 22
**Files likely touched:** `src/web/routes/redirect.rs`, `src/web/routes/transcript.rs`, `src/web/routes/run.rs`, `src/web/app.rs`, `src/commands/serve.rs`
**Estimated scope:** Medium

### Task 24: Cloudflare Access JWT middleware

**Description:** `web::auth`: middleware verifying `Cf-Access-Jwt-Assertion` with `jsonwebtoken` (RS256, `aud = CF_ACCESS_AUD`, issuer `https://<CF_ACCESS_TEAM>.cloudflareaccess.com`) against the team JWKS fetched with `reqwest` and cached 1 h, refetched on an unknown `kid`; 401 on failure, 503 while the JWKS cache is empty and unreachable; bypassed only when `CF_ACCESS_AUD` is unset **and** the bind is `127.0.0.1`. Applied to every route.

**Acceptance criteria:**
- [ ] test (wiremock JWKS + a locally generated RSA key): `auth_accepts_valid_assertion`; `auth_rejects_bad_signature`, `_wrong_audience`, `_expired`, `_missing_header`; `auth_refetches_on_unknown_kid`; `auth_503_when_jwks_unreachable_and_cache_empty`; `auth_bypassed_only_on_loopback_without_aud`; `post_run_is_401_without_assertion_when_aud_set`.

**Verification:**
- [ ] `cargo test web::auth`
- [ ] fast checks clean

**Dependencies:** Task 23
**Files likely touched:** `Cargo.toml` (`jsonwebtoken`, `rsa` dev-dep or a checked-in test key), `src/web/auth.rs`, `src/web/app.rs`
**Estimated scope:** Small

## Checkpoint G
- [ ] Every `SPEC.md` §7b route and status code tested; `cargo test` green
- [ ] Manual: real browser at phone width against a seeded temp DB (screenshot in the task note)
- [ ] Review with human before Phase 7

---

## Phase 7: Ops (`ops`)

### Task 25: `reembed` verb, `bin/check`, `deny.toml`, cargo profiles, gitleaks

**Description:** `commands::reembed` (every item and topic re-embedded in batches, progress on stderr); `bin/check fast|task|full` per `spec/r0.md` §4 with ADR 0004's two coverage reports; `deny.toml` with the approved licence allowlist and advisories; `[profile.dev.package."*"] opt-level = 2`, `[profile.release] lto = "thin", codegen-units = 1, strip = true`; install `gitleaks` on the box (approved) and run it.

**Acceptance criteria:**
- [ ] test: `reembed_rewrites_every_vector` (fake embedder, vectors change, count unchanged).
- [ ] `bin/check fast` ≤ 60 s warm on the host; `bin/check task` ≤ 4 min; `bin/check full` green including both coverage floors and `cargo deny check` and `gitleaks detect --redact --no-banner`.
- [ ] Wall times recorded in `CONSTRAINTS.md` "Measured".

**Verification:**
- [ ] `time bin/check fast`; `time bin/check task`; `bin/check full`

**Dependencies:** Task 24
**Files likely touched:** `Cargo.toml`, `src/commands/reembed.rs`, `bin/check`, `deny.toml`, `CONSTRAINTS.md`
**Estimated scope:** Medium

### Task 26: Dockerfile, `compose.yaml`, `bin/dc`, `.dockerignore`, compose test

**Description:** Multi-stage Dockerfile (`rust:1.98.1-bookworm` builder with BuildKit cache mounts → `debian:bookworm-slim` runtime with `ca-certificates`, Claude Code at an exact version via the native installer, uid 1000 user, `ENV` per `SPEC.md` §9, `COPY` of the binary, `prompts/`, `schemas/`, `config/`); `compose.yaml` profiles `app` (`127.0.0.1:8788:8788`, `env_file .env`, volumes `dailybrief-data:/data`, `dailybrief-claude:/home/app/.claude`, `init: true`, `restart: unless-stopped`, `CF_ACCESS_AUD` always set), `dev` (`cargo watch`), `test` (bind mount + `cargo-target` and `cargo-registry` volumes); `bin/dc`; `.dockerignore`; `tests/it/compose.rs`. Write ADR 0011 (Docker runtime, native vs npm Claude Code, with the verification result).

**Acceptance criteria:**
- [ ] test: `compose_publishes_loopback_only`; `compose_has_no_anthropic_keys_anywhere`; `compose_app_sets_cf_access_aud`; `dockerignore_excludes_env_data_target`.
- [ ] `bin/dc cargo test` green inside the `test` container; `docker compose build` succeeds; `docker run --rm dailybrief:runtime dailybrief --help` exits 0; `claude --version` inside the image prints the pinned version; `docker history dailybrief:runtime` shows no secret.
- [ ] `dailybrief fetch` inside the runtime image against a temp volume downloads the model and stores items (proves ort + glibc).

**Verification:**
- [ ] `docker compose build && bin/dc cargo test`
- [ ] the four manual commands above, outputs in the commit message

**Dependencies:** Task 25
**Files likely touched:** `Dockerfile`, `compose.yaml`, `.dockerignore`, `bin/dc`, `tests/it/compose.rs`, `docs/adr/0011-docker-runtime-setup-token.md`
**Estimated scope:** Medium

### Task 27: CI workflow

**Description:** `.github/workflows/ci.yml`: build the `test` image, run `bin/check full` inside it, upload the coverage JSON artifact, run `docker history` secret check on the runtime image, fail on any `src/<mod>/` directory matched by neither coverage regex (ADR 0004). Record `bin/check task` wall time in the log.

**Acceptance criteria:**
- [ ] CI green on `main` for the commit that adds it; the job log shows both coverage floors, `cargo deny`, `gitleaks`, and the wall time.

**Verification:**
- [ ] `gh run watch` on the push

**Dependencies:** Task 26
**Files likely touched:** `.github/workflows/ci.yml`, `bin/check`
**Estimated scope:** Small

## Checkpoint H
- [ ] `bin/dc cargo test` green; `bin/check full` green; CI green
- [ ] Image hygiene verified; `.env` holds only the OAuth token and the two `CF_ACCESS_*` values
- [ ] Review with human before spending subscription usage

---

## Phase 8: Ship

### Task 28: First deployment, smoke run, real digest run, ship ADRs, rollback rehearsal

**Description:** Copy the existing `brief.db` from the running data volume into `dailybrief-data` (`docker cp`), stop whatever listens on 8788, `dailybrief migrate` (no-op), `dailybrief reembed`, `dailybrief fetch`, the smoke run from `spec/r0.md` §4, then one real `dailybrief run`, then `docker compose --profile app up -d`. Rehearse rollback once (`down`, `up -d` on the previous image tag, `down`, `up -d` on the new one). Write ADRs 0006 (rmcp), 0007 (dom_smoothie), 0008 (rusqlite), 0009 (Access middleware in R0), 0010 (seed deferred). Update `CONSTRAINTS.md` "Measured" with the first coverage number and run turns/reads.

**Acceptance criteria:**
- [ ] `migrate` on the copied DB applies nothing and the file is byte-identical before `reembed`.
- [ ] Smoke transcript `system/init` lists `mcp_servers: [{ name: "dailybrief" }]` and only `WebSearch` + the nine `mcp__dailybrief__*` tools.
- [ ] Real run: `runs.status = success`, `digests` row with 24 + 6, `scan-transcript` exit 0; the `result` line attached to the ship commit.
- [ ] Rollback executed once and documented as one command in the ship note.
- [ ] Phone: `https://dailybrief.hundredclouds.com/` opens after OTP; one tap → `reads` row.

**Verification:**
- [ ] `spec/r0.md` §9 criteria 1–7, 10, 11 checked with evidence; 8 and 9 opened as a tracking note for three mornings

**Dependencies:** Task 27
**Files likely touched:** `docs/adr/0006-…` to `0010-…`, `CONSTRAINTS.md`, ship note
**Estimated scope:** Medium (docs + operations, no new code)

## Checkpoint: Complete
- [ ] All `spec/r0.md` §9 criteria met with evidence
- [ ] `/review` (code-reviewer + security-auditor) and `/ship` run
