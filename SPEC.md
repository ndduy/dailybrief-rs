<!-- Copied from the vault on 2026-09-17 (Build Spec — Daily Reading Agent (Rust), v4). The vault note is the source of truth for decisions; keep this copy in sync when decisions change, and record each change as an ADR (§0 SHIP phase). Wiki-links flattened to italics. -->


# Build Spec — Daily Reading Agent (v4, Rust, harness-agnostic, subscription-billed)

> **How to use this document.** Self-contained build spec for a coding agent working in a **fresh repository** (`dailybrief-rs`, not this vault). **Implementation method is mandated: the [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills) toolkit (§0) — install it first, then drive every milestone through its DEFINE → PLAN → BUILD → VERIFY → REVIEW → SHIP loop.** Decisions in §1 are settled. Two goals of equal weight: **ship a digest Duy reads**, and **practise building agents — now in Rust**. Billing constraint: the daily run must draw on a **subscription** (Claude Max, or a ChatGPT plan if Codex is chosen), not per-token API rates — which rules out writing the agent loop yourself for production (§2). So the service owns the **tools, contracts, invariants, storage, trajectories and page**, and a first-party harness (Claude Code or Codex) owns the loop. Build in the order of §8; each milestone is one agent-engineering skill plus one Rust skill. Anything marked *verify at build time* must be checked against installed versions (`cargo add` resolves the version; read the crate's docs.rs page for the exact API before using it).

**This document is self-contained.** The repo is a **clean build from this spec**: do not look for another implementation to port from or to diff against. Everything the build needs is here:

1. **Committed inputs**: `prompts/editor.md`, `prompts/smoke.md`, `schemas/digest.json`, `schemas/smoke.json`, `config/config.toml`, `config/feeds.toml`, `config/topics.toml`, `config/mcp.json`, `docs/references/*`, `.claude/` + `.agents/` (the toolkit install). Treat them as inputs; only `schemas/digest.json` is generated (from the Rust contract type, §7).
2. **Field-tested decisions, settled** in §1 (rows 14–18 came out of the first weeks of production).
3. **The SQLite DDL, verbatim in §5** — the service adopts an existing `brief.db` file on first deployment (§9), so the schema and the migration ledger must match the file.
4. **Behavioural contracts written out in full** (§4 tools, §5 scoring, §3 harness flags and runner policy, §7b web routes), precise enough to implement and test without another reference.

Parent project: *Daily Reading Agent* · Landscape survey: *Personalized Reading Agents — Open Source Landscape*. v0 (Rust, deterministic, 2026-09-08) was rejected because it had no agent; **this v4 is Rust *with* the agent loop in a first-party harness**.

---

## 0. Implementation toolkit — addyosmani/agent-skills (mandated)

The coding agent that builds this repo must work through the [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills) toolkit (MIT): 25 workflow skills, 4 reviewer personas, 9 slash commands, supported on Claude Code and Codex — so the same method applies whichever harness drives the build.

**Install (first action in the fresh repo):**

```bash
# universal installer (works for Claude Code and Codex)
npx skills add addyosmani/agent-skills
# or, in Claude Code
/plugin marketplace add addyosmani/agent-skills
/plugin install agent-skills@addy-agent-skills
```

**Prescribed loop per milestone:** `/spec` → `/plan` → `/build` → `/test` → `/review` → `/ship`. Use `/build auto` only after the plan for that milestone has been approved; it pauses on failures and risky steps.

**Skill mapping (which skills carry which part of this spec):**

| Phase | Skills to invoke | Applied to |
|---|---|---|
| DEFINE | `spec-driven-development`, `constraint-driven-development`, `interview-me` (only where this spec is silent) | Turn §3–§7 into `spec/r0.md`; write `CONSTRAINTS.md` from §1, §2 (subscription-only, no API key in service env), §7 (egress) and §11 — with the **cargo tooling numbers** in §7c |
| PLAN | `planning-and-task-breakdown` | One plan per milestone R0, M2–M8 (§8), each task verifiable against that milestone's gate; R0 tasks sliced by the module ids in §7b |
| BUILD | `incremental-implementation`, `test-driven-development`, `api-and-interface-design`, `source-driven-development`, `context-engineering`, `doubt-driven-development` | Thin vertical slices (feed → item → candidate → digest → page); contract-first for the MCP tool schemas (`schemars` structs are the contract) and `schemas/digest.json`; **every crate API grounded in docs.rs** (the *verify at build time* items — Rust crates move faster than the spec); `doubt-driven-development` on every `unwrap()`, every `unsafe` (there should be none) and every place the borrow checker is "worked around" with `clone()` |
| VERIFY | `debugging-and-error-recovery`, `browser-testing-with-devtools` | Failed-run triage from `run_events`; the full `cargo test` suite (fake harness, in-process MCP client) is green before any real harness spend; htmx rating flow and pages checked in a real browser |
| REVIEW | `code-review-and-quality`, `security-and-hardening`, `code-simplification`, `performance-optimization` | ~100-line change sizing; `cargo clippy -D warnings` clean; Access JWT middleware, secrets, egress; keep the service small (§7 non-goals); no `unsafe`, no `.unwrap()` outside tests and `main` |
| SHIP | `git-workflow-and-versioning`, `ci-cd-and-automation`, `documentation-and-adrs`, `observability-and-instrumentation`, `shipping-and-launch` | Trunk-based, atomic commits per task; CI runs `cargo test`, clippy, fmt, `cargo deny`, the M7 eval suite; ADRs for each §1 decision touched (§8 R0 lists the expected ones) and for every later harness/model change; `tracing` structured logs + RED metrics on `/runs`; launch checklist and rollback = redeploy the last good image tag |

**Non-negotiables inherited from the toolkit:** every skill ends with evidence (test output, build output, runtime data) — a milestone gate in §8 is met only with that evidence attached to the ship commit; anti-rationalization tables apply (no skipping tests for "just wiring"). Use the `code-reviewer` and `security-auditor` personas before each `/ship`.

## 1. Settled decisions

| # | Decision | Consequence |
|---|---|---|
| 1 | Topics seeded from vault Areas (sensitivity-marked Areas excluded); **no X/Twitter** | `config/topics.toml` is committed (already filtered and hand-edited); the `seed` command exists for completeness but is not re-run |
| 2 | **dailybrief.hundredclouds.com**, auth = artifacts pattern (localhost bind → cloudflared tunnel → Cloudflare Access OTP) | Service on `127.0.0.1:8788` behind the existing tunnel hostname and Access app |
| 3 | **30 items** with source links; **fixed morning run + on demand** | 24 for-you + 6 beyond-radar; on-demand run dedupes against everything shown today |
| 4 | **Rating with reason** | Fixed reason set (§6); reasons feed the weekly Curator |
| 5 | **20% exploration** | 6 slots, reason always printed |
| 6 | **Standalone** — vault is a seed, never a runtime dependency | Profile lives in the service's SQLite |
| 7 | Egress = as (2); only public article text and topic *names/one-liners* reach any model | Embeddings computed locally |
| 8 | Standalone service, own scheduler and process | No `/loop` or Queue.md coupling |
| 9 | **Rust** (2026-09-17) | §7 stack; single static-ish binary |
| 10 | Success metric = **items read** (`/r/{id}` redirect logs the click) | `/stats` shows reads/day, 7-day rate |
| 11 | **Agent loop only — no deterministic fallback** | Failed run → retried once → shown as failed with its transcript; Refresh button is the manual retry |
| 12 | **Subscription billing, not API rates** | Production loop runs in **Claude Code** (Max) or **Codex** (ChatGPT plan); own-loop code is a low-volume side quest on an API key (§8 M6) |
| 13 | **Harness-agnostic service** | Tools over MCP, prompts and schemas as files, harness = config switch; trajectories ingested from either harness's event stream |
| 14 | **SQLite schema fixed by §5; the existing `brief.db` is adopted, not recreated** | `migrate` adopts the existing migration ledger (DDL verbatim in §5); the file is copied into the data volume on first deployment and its rows are read as-is; vectors are re-embedded once with `dailybrief reembed` (§7 Embeddings) |
| 15 | **Harness isolation flags** | `--restricted --tools WebSearch --strict-mcp-config --permission-prompts none --max-turns 120 --no-session-persistence`, never `--bare` — §3 |
| 16 | **Editor prompt is the system prompt** | `--system-prompt-file prompts/editor.md`; fixed user message "Build today's digest. Start with get_briefing."; a test asserts the prompt bytes contain no date or id |
| 17 | **"Read before you select" enforced by the tools** | `select` refuses an item not passed to `read_item` in the same run (`run_reads` row); `publish_digest` re-checks; staging is in SQLite keyed by run id so the MCP process is stateless |
| 18 | **Everything in Docker; `claude` in the container on a `claude setup-token` OAuth token** | `CLAUDE_CODE_OAUTH_TOKEN` in the git-ignored `.env` is the only model credential; never baked into an image; the env allowlist (§3) refuses `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` |

## 2. Billing and harness facts (verified 2026-09-08; setup-token path verified in production 2026-09-12 → 09-17)

**Claude.**
- Agent SDK overview: *"Unless previously approved, Anthropic does not allow third party developers to offer claude.ai login or rate limits for their products, including agents built on the Claude Agent SDK. Use the API key authentication methods described in the Quickstart instead."* → the Agent SDK, the tool runner and any hand-written Messages-API loop are **API-billed**. Not for production here. (There is also **no official Rust SDK** — the M6 side quest is raw HTTP by necessity, §8.)
- Claude Code itself is the permitted place for Max credentials, including headless mode. Authentication precedence: `ANTHROPIC_API_KEY` → `apiKeyHelper` → profile / WIF → subscription OAuth. `claude setup-token` mints a one-year OAuth token for `CLAUDE_CODE_OAUTH_TOKEN`, billed to the subscription, meant for scripts and containers, **not read in `--bare` mode**. So: **use normal `claude -p`, never `--bare`**, and keep `ANTHROPIC_API_KEY` out of the service environment (it outranks the subscription).
- Verified flags: `--restricted` (v2.1.248+) ignores user/project/local settings and removes code-running tools; `--tools` restricts built-ins; `--strict-mcp-config` drops every MCP server not in `--mcp-config`; `--permission-prompts none` (v2.1.259+) denies without retrying; `--max-turns` is print-mode only and documented although hidden from `--help`; `--json-schema` enforces the final message shape; `--output-format stream-json --verbose` emits one JSON object per line.
- Observed usage (six production mornings): 86–91 tool round-trips per run at one call per message; a 60-turn cap made retry the normal path, hence **120**. Wall clock 15 min and the 45-read cap remain the effective bounds. Schedule the morning run when you are not using Claude Code interactively.
- Routines (cloud scheduled agents) are available on Max and draw down subscription usage; they reach a self-hosted MCP server only over the network (through the tunnel with a service token). Kept as M8.

**Codex (alternative harness).** `codex exec` reuses saved CLI authentication; `codex exec --json` emits JSONL; `--output-schema` forces the final answer into a JSON Schema; tools are MCP servers in Codex config (`required = true` makes a missing server fatal). Cost of choosing it: a second subscription, less transfer to the vault agent team.

**Consequence.** Both harnesses consume the same three things from the service: an **MCP server**, a **prompt file**, and an **output schema**. That is the whole integration surface, so the harness is a config value.

## 3. Architecture

```
 config/{config,feeds,topics}.toml · config/mcp.json · prompts/{editor,curator}.md · schemas/digest.json   (committed)
 SQLite /data/brief.db (schema in §5): sources, items, topics, runs, run_events, run_reads,
        selections, digests, digest_items, reads, editor_notes, run_lock, feed_issues (+ M3: ratings, proposals)
        │
        │  scheduler 06:30 Asia/Ho_Chi_Minh / POST /run          (one tokio process, always on)
        ▼
 HARNESS ADAPTER  (trait Harness)  ──  spawn one of:
   claude-code : claude -p "Build today's digest. Start with get_briefing."
                 --system-prompt-file prompts/editor.md --restricted --tools WebSearch
                 --strict-mcp-config --mcp-config <run dir>/mcp.json
                 --allowedTools "mcp__dailybrief__*" WebSearch --permission-prompts none
                 --max-turns 120 --model opus --output-format stream-json --verbose
                 --json-schema <schemas/digest.json contents> --no-session-persistence   (Max, setup-token)
   codex       : codex exec --json --output-schema schemas/digest.json -c mcp_servers…
                 --sandbox read-only <prompt>                                             (ChatGPT plan, M4)
   raw-loop    : own loop, reqwest → POST /v1/messages on an API key — M6 side quest only, never scheduled
        │
        │  stdio (tokio child process; env = allowlist only)
        ▼
 MCP SERVER  `dailybrief mcp`  (same binary; rmcp `#[tool_router]` over schemars structs)
   editor tools : get_briefing · fetch_sources · list_candidates · read_item · search_items ·
                  select · publish_digest · report_feed_issue · editor_notes
   curator tools: get_feedback · get_profile · find_feeds · validate_feed · propose_change      (M3)
        │
        ▼
 CORE (plain Rust modules)  fetch (reqwest, conditional GET) → extract (dom_smoothie) → dedupe →
                             embed (fastembed, local ONNX) → score signal → store (rusqlite)
        │
        ▼
 WEB (axum + maud + htmx)  /  ·  /d/{date}  ·  /runs/{id}  ·  /curator  ·  /stats  ·  /feeds  ·  /topics  ·  /r/{id}  ·  POST /run
```

**Division of labour.** Code owns fetching, extraction, dedupe, embeddings, the similarity *signal*, storage, validation, trajectory capture and serving. The harness owns the loop, context management and retries within a turn. The agent owns judgement: what makes the cut and why, explore reasons, summaries, spotting broken sources, weekly proposals. Every state change beyond today's digest passes through the approval queue, enforced by the tool itself (`propose_change` only ever writes a proposal row).

**Web search for explore evidence** is the harness's own tool (`WebSearch`), allowlisted for the Editor with a cap in the prompt, and reported in the trajectory. The service does not proxy it.

**Spawn environment (allowlist, enforced by a unit test):** `PATH HOME LANG TZ CLAUDE_CONFIG_DIR CLAUDE_CODE_OAUTH_TOKEN DISABLE_AUTOUPDATER CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC DAILYBRIEF_*`. The adapter **refuses to start** if the service env holds `ANTHROPIC_API_KEY` or `ANTHROPIC_AUTH_TOKEN`, or lacks the OAuth token. The per-run `mcp.json` is written to `<data>/runs/<id>/` from the template `config/mcp.json` with `${MCP_COMMAND}` = the running binary's own path (`std::env::current_exe()`), `${MCP_ARGS}` = `["mcp"]`, plus `DAILYBRIEF_RUN_ID`, `DAILYBRIEF_CONFIG`, `DAILYBRIEF_DATA_DIR`.

**Failure policy.** Turn cap (harness flag) and wall-clock cap (adapter kills the child after 15 min — `tokio::time::timeout` around `child.wait()`, then `kill_on_drop`); `publish_digest` rejected 3× ends the run as failed. One retry from a fresh context; then the day shows "No digest — run failed" with the transcript link and the Refresh button. The raw `stream-json` transcript is always written to `<data>/runs/<id>/transcript.jsonl` before parsing, so a parser bug never loses a run.

**Runner policy, precisely.** Run id = `YYYY-MM-DD-<8 hex>` (local date + the first 8 hex chars of a UUID v4). Each attempt gets its own run id, its own `<data>/runs/<id>/` directory (the child's working directory — no `CLAUDE.md`, `.mcp.json` or `.claude/` inside), its own rendered `mcp.json` and an empty `transcript.jsonl` created before the spawn. A `runs` row is inserted with `status = running` before the spawn and finished with `status ∈ {success, failed, killed}`, `ended_at`, `turns` (= `result.num_turns`), `usage_json`, `cost_usd` (= `total_cost_usd`, informational), `session_id`, `error`. Every stdout line is appended to the transcript file and to `run_events(run_id, seq, type, payload_json)` with `type` = the line's `type` field or `unparseable`. Outcome mapping: wall clock hit → `killed` (SIGTERM, SIGKILL after 10 s); `result.subtype == "success"` and `!is_error` → candidate success; any other `result` → `failed` with `result.subtype` (or its `result` text) as the error; exit without a `result` line → `failed` (`no_result` or the exit code plus the last 4 kB of stderr). A candidate success counts only when `structured_output` validates against `schemas/digest.json` **and** a `digests` row exists for this run id **and** its id equals `structured_output.digestId`; otherwise it is `failed` with the reason. `run_lock` is a single row acquired before attempt 1 and released after the last attempt; a lock older than `2 × wall_clock + 5` minutes is stale and may be taken over. Before each run the service mirrors `topics.toml` into `topics` (§5). `POST /run` and the scheduler share this code path.

## 4. Editor contract (files committed, not rewritten)

**Prompt** (`prompts/editor.md`, 5.8 kB, static, byte-stable): role, the two goals, reason taxonomy, digest contract (24 + 6, ≤ 4 items per source, ≤ 8 per topic, nothing shown in the last 14 days, every beyond-radar item carries a reason), "read before you select", web search only for explore evidence and at most 5 calls, what to do when a tool errors, final-message shape. Dynamic context is **not** in the prompt: the Editor's first tool call is `get_briefing()`.

**Expected trajectory** (observed 86–91 round-trips): `get_briefing` → `fetch_sources` → `list_candidates(exploit, 80)` ‖ `list_candidates(popular_unmatched, 30)` ‖ `list_candidates(cold_topic, 10)` → `read_item` ×35–45 → `search_items` / web search only when an explore slot lacks a candidate → `select` ×30 → `publish_digest` → final message conforming to `schemas/digest.json` (`digestId`, `date`, `forYou`, `beyondRadar`, optional `notes`).

**Tools** (MCP; each input is a `#[derive(Deserialize, JsonSchema)]` struct, each result a `#[derive(Serialize, JsonSchema)]` struct returned as compact JSON text **and** `structuredContent`, never HTML):

| Tool | Does | Returns |
|---|---|---|
| `get_briefing()` | Dynamic context in one call | date, profile L0, shownIds, feedHealth, notes |
| `fetch_sources()` | Pull enabled feeds (conditional GET via `etag` / `last_modified`), extract, dedupe (URL/title hash + cosine > 0.92 vs 14 d), embed | counts per feed, failures |
| `list_candidates({strategy, limit})` | `exploit`: weighted-mean cosine vs profile (§5); `popular_unmatched`: social ≥ p80 and exploit ≤ p40; `cold_topic`: nearest to seed topics with no positive signal in 30 d | `[{id, score, title, source, published, snippet}]` |
| `read_item({id})` | Extracted text, capped 5k chars; writes a `run_reads` row; counts toward the 45-read cap (error past it) | text, wordCount |
| `search_items({query, limit})` | Embedding search over the last 7 days | candidates shape |
| `select({section, itemId, summary, whyItMatters, reason, topic})` | Stage into `for_you` / `beyond_radar` (`section: "none"` un-stages); refuses unread items; `summary ≤ 80 words`, `whyItMatters ≤ 25`, `reason` required for `beyond_radar`; caps checked incrementally | ok / validation error |
| `publish_digest()` | Validate the staged set end-to-end (counts, caps, read-before-select, no repeats in 14 d), write `digests` + `digest_items`, return the id | ok / violations |
| `report_feed_issue({feedId, kind, note})` | Record dead / paywalled / junk for the Curator (`feed_issues`) | ok |
| `editor_notes({op, text})` | Read/replace the ≤ 2k-char `editor_notes` row — harness-independent memory | notes |

Tool names are `snake_case` and field names are **camelCase on the wire** (the prompt refers to them by these names) — `#[serde(rename_all = "camelCase")]` on every tool struct; a test drives `tools/list` through an in-process rmcp client and asserts exactly the nine names above with the input fields named as in this table. Tool descriptions are written fresh, one or two sentences each, in the imperative voice the prompt uses.

**Tool contracts, precisely** (the wire shapes; `caps` values come from `config/config.toml [caps]`):

- Every result is `content: [{type: "text", text: <compact JSON>}]` plus `structuredContent` with the same object. A contract violation is `isError: true` with `{"error": "<one imperative sentence>"}`; a structured rejection the agent must fix (publish violations) keeps its full object and also sets `isError: true`. A tool that panics or hits an I/O error returns `isError` with one sentence, never a stack trace. No tool returns HTML, and no tool other than `read_item` returns article text.
- `get_briefing()` → `{ date, timezone, profile: { topics: [{ name, weight }] }, shownIds: [id], feedHealth: [{ source, lastOk, failures }], notes, caps: { reads, forYou, beyondRadar, perSource, perTopic, webSearch } }`. Topic descriptions are never included (egress, §1 #7).
- `fetch_sources()` → `{ fetched, newItems, perFeed: [{ source, status, newItems, error? }] }`. Idempotent within a run. Total budget `ingest.total_budget_ms`, concurrency `ingest.concurrency`, per-request timeout `ingest.request_timeout_ms`, body cap `ingest.body_max_bytes`, http(s) only, ≤ `ingest.max_redirects`. A feed failure increments `sources.failures` and sets `last_error`; success resets `failures`, sets `last_ok_at`, stores `etag`/`last_modified`; a 304 is a success with zero new items.
- `list_candidates({ strategy, limit })` → `{ candidates: [{ id, score, title, source, published, snippet, note? }] }` per §5.
- `read_item({ id })` → `{ id, title, source, text, wordCount, truncated }`; `text` is the first `read_text_chars` characters. Unknown id → error. The `caps.reads`-th distinct id is the last one allowed; re-reading an already-read id is free. Writes `run_reads(run_id, item_id)`.
- `search_items({ query, limit })` → same shape as `list_candidates`.
- `select({ section, itemId, summary, whyItMatters, reason?, topic })`, `section ∈ {for_you, beyond_radar, none}` → `{ ok: true, replaced, staged: { forYou, beyondRadar } }`. `none` deletes the staged row for `itemId` and never errors. Rejections, checked in this order, each one sentence naming the value and the cap: unknown item; item not in `run_reads` for this run; empty `summary` / `whyItMatters`; `summary` > 80 words; `whyItMatters` > 25 words; `topic` not a known topic *name* (case-insensitive; the message lists the known names); `beyond_radar` without a `reason` or with a reason outside `{new_to_me, adjacent_field, contrarian, deep_dive, emerging}`; item in `shownIds`; a `perSource`-th other item from the same source already staged; a `perTopic`-th other item in the same topic; the section already at its cap. Re-selecting a staged id replaces its row (keeps its position when the section is unchanged). Staging lives in `selections` keyed by run id, so the MCP process is stateless and a retry starts empty.
- `publish_digest()` → `{ ok: true, digestId, date, forYou, beyondRadar }` or `{ ok: false, violations: [sentence], fatal? }`. Violations: `for_you` count ≠ `caps.forYou`; `beyond_radar` count ≠ `caps.beyondRadar`; a staged item that no longer exists, was not read, or is in `shownIds`; a `beyond_radar` item without a valid reason; any source over `perSource`; any topic over `perTopic`; the run already published. The third rejection in one run adds `fatal: true`; the runner then treats the run as failed when the transcript ends without a digest. On success: `digestId = <local date>-<run id>`, `digests` + `digest_items` written in one transaction, positions renumbered 1… per section in staged order.
- `report_feed_issue({ feedId, kind, note? })`, `kind ∈ {dead, paywalled, junk, duplicate}` → `{ ok: true }`; writes `feed_issues` and `sources.last_error`. Unknown feed → error.
- `editor_notes({ op, text? })`, `op ∈ {read, replace}` → `{ notes }`; `replace` requires `text` ≤ 2 000 chars and writes the single `editor_notes` row.

## 5. Storage, profile, scoring signal, Curator

**Schema (§1 #14).** The migration ledger is `migrations(id TEXT PRIMARY KEY, applied_at TEXT NOT NULL)`, created with `CREATE TABLE IF NOT EXISTS` before it is read. Migrations are applied in id order, each inside one transaction that runs the SQL and inserts the ledger row (`applied_at` = RFC 3339 UTC); an applied migration is never edited. The live database already holds `0001_init`, so `migrate` must be a no-op on it (a test proves that by applying the ledger twice). M3 appends `0002_feedback` (`ratings(item_id, sign, reason, at)`, `proposals(id, kind, payload_json, evidence_json, status)`) to the same ledger. `0001_init` is exactly:

```sql
CREATE TABLE sources (
  id TEXT PRIMARY KEY,
  url TEXT NOT NULL,
  title TEXT NOT NULL,
  weight REAL NOT NULL DEFAULT 1,
  enabled INTEGER NOT NULL DEFAULT 1,
  etag TEXT,
  last_modified TEXT,
  failures INTEGER NOT NULL DEFAULT 0,
  last_ok_at TEXT,
  last_error TEXT
);

CREATE TABLE items (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  url TEXT NOT NULL,
  canonical_url TEXT NOT NULL UNIQUE,
  title TEXT NOT NULL,
  author TEXT,
  published_at TEXT,
  fetched_at TEXT NOT NULL,
  text TEXT NOT NULL,
  word_count INTEGER NOT NULL,
  content_hash TEXT NOT NULL,
  title_hash TEXT NOT NULL,
  vector BLOB,
  social_score REAL
);
CREATE INDEX items_fetched_at ON items(fetched_at);
CREATE INDEX items_title_hash ON items(title_hash);

CREATE TABLE topics (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  weight REAL NOT NULL DEFAULT 1,
  vector BLOB,
  origin TEXT NOT NULL CHECK (origin IN ('seed', 'curator', 'explore-promoted')),
  last_positive_at TEXT,
  saturation INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE runs (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('scheduled', 'manual')),
  harness TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('running', 'success', 'failed', 'killed')),
  attempt INTEGER NOT NULL DEFAULT 1,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  turns INTEGER,
  usage_json TEXT,
  cost_usd REAL,
  session_id TEXT,
  error TEXT,
  transcript_path TEXT
);

CREATE TABLE run_events (
  run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  PRIMARY KEY (run_id, seq)
);

CREATE TABLE run_reads (
  run_id TEXT NOT NULL,
  item_id TEXT NOT NULL REFERENCES items(id),
  PRIMARY KEY (run_id, item_id)
);

CREATE TABLE selections (
  run_id TEXT NOT NULL,
  item_id TEXT NOT NULL REFERENCES items(id),
  section TEXT NOT NULL CHECK (section IN ('for_you', 'beyond_radar')),
  position INTEGER NOT NULL,
  summary TEXT NOT NULL,
  why_it_matters TEXT NOT NULL,
  reason TEXT,
  topic TEXT NOT NULL,
  PRIMARY KEY (run_id, item_id)
);

CREATE TABLE digests (
  id TEXT PRIMARY KEY,
  date TEXT NOT NULL,
  run_id TEXT NOT NULL REFERENCES runs(id),
  published_at TEXT NOT NULL,
  for_you_count INTEGER NOT NULL,
  beyond_radar_count INTEGER NOT NULL
);
CREATE INDEX digests_date ON digests(date);

CREATE TABLE digest_items (
  digest_id TEXT NOT NULL REFERENCES digests(id) ON DELETE CASCADE,
  item_id TEXT NOT NULL REFERENCES items(id),
  section TEXT NOT NULL CHECK (section IN ('for_you', 'beyond_radar')),
  position INTEGER NOT NULL,
  summary TEXT NOT NULL,
  why_it_matters TEXT NOT NULL,
  reason TEXT,
  topic TEXT NOT NULL,
  PRIMARY KEY (digest_id, item_id)
);

CREATE TABLE reads (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  item_id TEXT NOT NULL REFERENCES items(id),
  digest_id TEXT REFERENCES digests(id),
  at TEXT NOT NULL
);

CREATE TABLE editor_notes (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  text TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL
);

CREATE TABLE run_lock (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  run_id TEXT NOT NULL,
  acquired_at TEXT NOT NULL
);

CREATE TABLE feed_issues (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  source_id TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  run_id TEXT,
  kind TEXT NOT NULL,
  note TEXT,
  at TEXT NOT NULL
);
```

`shownIds(14 d)` = every `digest_items.item_id` whose digest was published in the last `shown_days`. `topics` is mirrored from `config/topics.toml` before every run (upsert by id, delete `seed` rows no longer in the file) and each topic's vector is embedded locally from `name + ". " + description` (or `name` alone when the description is empty).

**Vectors**: `BLOB` of 384 little-endian `f32`. Read with `bytemuck::cast_slice` or a chunked `f32::from_le_bytes`; write the same way. Brute-force cosine over ≤ 20k rows in a `spawn_blocking` closure — milliseconds; no vector DB.

**Seed**: `dailybrief seed --vault <ro mount> --out config/topics.draft.toml` ported for completeness (skip any Area with `sensitivity:`; `name`, `weight = source_count`, `description` = the `index.md` one-liner). Not re-run: `config/topics.toml` is committed.

**Exploit signal**: profile = topic vectors (`w_t`) ∪ rated/read item vectors (+1 👍, +0.5 read, −1 👎 reason-adjusted, each × `1/(1+log10(rank))` recency decay); `score = Σ w·cos / Σ|w|`. Topics with `saturation ≥ 3` damped ×0.5; topics with 3 "off-topic" votes excluded. A number next to each candidate — it ranks nothing by itself.

**Candidate strategies, precisely** (R0; ratings do not exist until M3, so the profile is topics + reads):
- *Profile vectors*: every topic with a vector, weight `topics.weight` (× 0.5 when `saturation ≥ 3`); the 200 most recent `reads` that have an item vector, weight `0.5 / (1 + log10(rank))` with rank 1 = most recent. Score of an item = `Σ w·cos(item, v) / Σ|w|`, 0 when the profile is empty.
- *Pool*: items with a vector, fetched in the last `candidate_days` and `published_at` (or `fetched_at` when null) inside that window, not in `shownIds`.
- `exploit`: pool sorted by score desc, then id asc.
- `cold_topic`: topics with a vector whose `last_positive_at` is null or older than 30 days; score = max cosine of the item to any of them; the exploit top-80 ids are excluded; sort score desc, id asc.
- `popular_unmatched` (no social signal exists yet): items whose exploit score ≤ the 40th percentile of the pool, sorted by source weight desc, then `published_at`/`fetched_at` desc, then id asc; every row carries `note: "no social signal yet: low-exploit items ordered by source weight, then recency"`.
- `limit` is clamped to 1…100; `snippet` = the first 200 characters of whitespace-collapsed text with `…` when cut.
- `search_items`: embed the query locally, cosine over the same window (shown items included — the Editor sees `shownIds`), sort score desc then id asc, `limit` clamped to 1…30.

**Curator** (M3, weekly, same harness, `prompts/curator.md`). Input via `get_feedback` (7 days of ratings with reasons, reads, explore hit-rate, feed issues) and `get_profile`. Harness web search (cap 8, queries from topic *names* only); `find_feeds({topic})` + `validate_feed({url})` (fetch, parse with `feed-rs`, items/day); `propose_change({kind, payload, evidence})` with kinds `topic_weight`, `add_topic`, `disable_source`, `add_source`, `promote_explore_topic`. **The tool writes a proposal row and nothing else.** `/curator` shows each with its evidence; approve/reject applies or discards.

## 6. Feedback: rating with reason (M3, unchanged)

| Rating | Reasons | Immediate effect (code) | Curator input |
|---|---|---|---|
| 👍 | New to me · Deep / actionable · Relevant to current work · Good source | +1 item vector; *good source* → source weight +0.2 | *new to me* ×2 on an explore topic → proposal to promote |
| 👎 | Already know this · Off-topic · Low quality / clickbait · Too shallow | *already know* → topic saturation +1; *off-topic* → −1 and topic counter; *low quality* → source −0.3; *too shallow* → −0.5 | repeated *off-topic* → lower topic weight; repeated *low quality* → disable source |

UI: two buttons per item, reasons revealed inline (htmx), one click completes, un-rating allowed, no free text.

## 7. Stack (Rust, minimum owned surface)

Versions are what crates.io reported stable on 2026-09-17; `cargo add` will pick the current one — **read the docs.rs page of the resolved version before writing against it** (`source-driven-development`).

| Concern | Choice | Version seen | Why / notes |
|---|---|---|---|
| Toolchain | stable Rust, **edition 2024**, pinned in `rust-toolchain.toml`; `cargo` workspace-free single package (`src/lib.rs` + `src/main.rs`) | current stable | Module tree in §7b; split into a workspace only if incremental compile exceeds ~20 s |
| Async runtime | `tokio` (full) | 1.x | One runtime for the scheduler, web server, child processes and MCP server |
| MCP server | **`rmcp`** (official Rust SDK, `features = ["server", "transport-io"]`; `transport-streamable-http-server` only for M8) with `#[tool_router]` / `#[tool]` macros and `schemars` for input/output schemas | 3.4 | The one integration surface both harnesses speak; in-process tests drive the server through an rmcp **client** over a `tokio::io::duplex` pair (*verify the transport constructor*) |
| Schemas | `schemars` 1 (+ `serde`) — tool params, tool results, `DigestOutput`; `cargo run --bin gen-schemas -- --check` regenerates `schemas/digest.json` (**draft-07**, `additionalProperties: false`) and fails CI on drift | 1.2 | "Generated schema matches the committed file" is a CI gate; schemars 1 emits 2020-12 by default — set the draft-07 target so Claude Code's `--json-schema` accepts it (*verify*) |
| Harness adapters | `tokio::process::Command` (`kill_on_drop(true)`, `env_clear()` + allowlist), `stream-json` parsed line-by-line with `serde_json` into an internally-tagged enum (`type`: `system`/`assistant`/`user`/`result`, `subtype`: `init`, `success`, `error_max_turns`, `error_during_execution` — *verify the event catalogue against the Agent SDK docs and the first smoke-run transcript this repo produces*; unknown `type` values pass through untouched) | — | Loop stays first-party and subscription-billed |
| Own-loop side quest (M6) | `reqwest` (rustls) → `POST https://api.anthropic.com/v1/messages`, headers `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`; body per §8 M6 | 0.13 | **No official Rust SDK exists** — raw HTTP is the documented path for unsupported languages; typed with hand-written `serde` structs for exactly the fields used |
| Web | **`axum`** 0.8 + `tower-http` (trace, compression) + **`maud`** compile-time templates + htmx via inline `<script>` from cdnjs | 0.8 | Type-checked HTML like `hono/jsx`; no frontend build. (`askama` 0.16 is the fallback if maud's macro ergonomics grate) |
| DB | **`rusqlite`** (`features = ["bundled"]`, WAL, `PRAGMA foreign_keys = ON`), raw SQL in one `db::repo` module; one `Connection` behind `Arc<Mutex<_>>`, every call inside `tokio::task::spawn_blocking` | 0.40 | ≤ 20k rows; a single writer is the point. `sqlx` rejected: compile-time query checking needs a DB at build time and the async pool buys nothing here |
| Scheduler | `tokio-cron-scheduler` with the `Asia/Ho_Chi_Minh` timezone (`chrono-tz`) | 0.15 | Two cron lines (morning run; weekly curate). *Verify the tz-aware job constructor name* |
| HTTP fetch | `reqwest` (rustls, gzip/brotli, `timeout`, `redirect::Policy::limited(3)`), body cap 2 MiB via streamed read | 0.13 | Conditional GET with `If-None-Match` / `If-Modified-Since` from the `sources` row |
| Feeds | **`feed-rs`** (RSS 0.9–2.0, Atom, JSON Feed) | 2.4 | One parser for every feed format |
| Extraction | **`dom_smoothie`** (Readability port, returns title/byline/text/word count); fallback `readability` 0.3 | 0.18 | ADR. Tested on a few sample pages committed under `tests/fixtures/html/` and written for this repo (a long article, a short one, a page with no article body): title, byline, body text and word count; a page with no extractable body yields empty text and the item is skipped, never stored |
| Embeddings | **`fastembed`** — `TextEmbedding::try_new(TextInitOptions::new(EmbeddingModel::BGESmallENV15).with_cache_dir(<models dir>))`, 384-d, normalised | 7.0 | `dailybrief reembed` re-embeds every item and topic once on first deployment (§9), so whatever vectors the adopted database holds are replaced, never compared → ADR. `Embedder` is a trait; unit tests use a deterministic fake (hash → unit vector) so the model never loads in the fast loop; one opt-in test (`EMBED_REAL=1`) loads the real model. Pulls `ort` (ONNX Runtime) — the first `fetch` downloads ~130 MB into the data volume |
| Auth | Cloudflare Access in front; axum middleware verifies `Cf-Access-Jwt-Assertion` with **`jsonwebtoken`** 11 against the team JWKS (`https://<team>.cloudflareaccess.com/cdn-cgi/access/certs`, cached 1 h), audience `CF_ACCESS_AUD` | 11.1 | Same trust boundary as artifacts, plus defence in depth (pulled forward from M5 into R0: `POST /run` is a billed action and must not be reachable unauthenticated). `POST /run` additionally requires a same-origin request and is capped at 3 manual runs per rolling 24 h |
| Config / secrets | `toml` 1 + `serde` for `config/config.toml`, `config/feeds.toml`, `config/topics.toml` (validated in `TryFrom` impls; `feeds.toml` and `topics.toml` are resolved next to `config.toml`); `CF_ACCESS_AUD`, `CF_ACCESS_TEAM`, `CLAUDE_CODE_OAUTH_TOKEN` from env; **no `ANTHROPIC_API_KEY` in the service env** | 1.1 | Keys exactly as in the committed `config/config.toml`; the values shown there are the defaults for omitted keys; unknown keys are an error |
| CLI | `clap` 4 derive: `serve`, `run [--harness] [--attempts] [--no-verify] [--prompt] [--schema] [--message]`, `fetch`, `seed`, `mcp`, `migrate`, `curate` (M3), `scan-transcript`, `reembed`, `gen-schemas` | 4.6 | One verb per operator action; the runbook (§9) uses them |
| Errors / logs | `thiserror` in library modules, `anyhow` only in `main`; `tracing` + `tracing-subscriber` (JSON to stdout → `docker logs` / journald) | — | Every tool error returned to the agent is a typed enum rendered to a short message, never a stack trace |
| Ops | Docker image (§9), compose profiles `app`/`dev`/`test`, cloudflared on the host, user-level systemd unit wrapping `docker compose` | — | Deploy and rollback are one `docker compose` down/up each (§9) |

Non-goals: multi-user, mobile app, vector DB, queue, agent frameworks, `async-trait` towers of abstraction, `unsafe`.

### 7a. Rust-specific rules (write these into `CONSTRAINTS.md`)

- **No `unwrap()` / `expect()` outside `#[cfg(test)]` and `main.rs`**; no `unsafe`; `#![deny(clippy::all)]` with `-D warnings` in CI. `clippy::pedantic` is advisory, read at `/review`.
- **Tool results are values, not strings.** Each MCP tool returns `Result<T: Serialize + JsonSchema, ToolError>`; the rmcp layer serialises. `ToolError` has a variant per contract violation (`Unread`, `CapExceeded { cap, have }`, `ShownRecently`, `BadSection`, …) so the agent sees the same wording every time — the prompt quotes them.
- **All SQLite access goes through `db::repo`** — typed functions, `rusqlite::params!`, no SQL strings elsewhere; every function unit-tested against an in-memory DB migrated with the real ledger.
- **Blocking work off the runtime**: rusqlite, fastembed, dom_smoothie all run inside `spawn_blocking`; a clippy-adjacent test greps for direct `Connection` use outside `db::`.
- **Byte-stable prompt**: a test asserts `prompts/editor.md` contains no `\d{4}-\d{2}-\d{2}` and no UUID-shaped string.
- **Time**: `chrono` + `chrono-tz`; every stored timestamp is RFC 3339 UTC with millisecond precision and a `Z` suffix (the format already in the live database, e.g. `2026-09-17T06:30:00.000Z`), so string comparison orders them; `date` on digests is the local `Asia/Ho_Chi_Minh` day.

### 7b. Module layout

```
src/
  main.rs            clap → commands::*
  lib.rs             pub mod …
  commands/          serve, run, fetch, seed, mcp, migrate, curate, scan_transcript, reembed, gen_schemas
  config/            schema.rs (serde structs + TryFrom validation), load.rs
  db/                migrate.rs (ledger), migrations.rs (the §5 DDL as a const), repo.rs (all SQL)
  core/              http.rs, fetch.rs, extract.rs, dedupe.rs, embed.rs, ingest.rs, profile.rs,
                     candidates.rs, staging.rs, digest.rs, time.rs
  mcp/               server.rs (tool_router), result.rs, tools/{get_briefing,fetch_sources,list_candidates,
                     read_item,search_items,select,publish_digest,report_feed_issue,editor_notes}.rs
  editor/            digest_output.rs (DigestOutput + JsonSchema), prompt.rs (byte-stability test)
  harness/           types.rs (trait Harness, HarnessRequest, RunOutcome), claude_code.rs (argv, env
                     allowlist, stream-json enum), codex.rs (M4), raw_loop.rs (M6), runner.rs (attempts,
                     wall clock, lock), service_runner.rs (scheduler + POST /run), verify.rs, scan_transcript.rs
  web/               app.rs (router, Access middleware), views/{today,day,run,curator,stats,feeds,topics}.rs
tests/               compose.rs (parses compose.yaml: 127.0.0.1 bind, no API key)
tests/fixtures/      feeds/ (rss2.xml, atom.xml), html/ (sample articles), transcripts/ (stream-json recorded by
                     this repo's own smoke runs + hand-written failure cases) — all produced here, none imported
tests/fake-claude/   an executable that replays a transcript file; on PATH in adapter tests
```

**Web routes, precisely (R0).** `/` = `/d/{today in service.timezone}`. `/d/{date}` renders the latest published digest for that date, else the latest run of that day in its state ("No digest — run failed" with the error summary, transcript link and Refresh button; "Run in progress"; "No run yet"). Item card: title linking to `/r/{id}`, source, published date, summary, why-it-matters, topic; `beyond_radar` cards carry the reason as a badge; two sections, 24 then 6; mobile-first, system fonts, ≤ 15 kB HTML, no client JS except htmx for Refresh. `/r/{id}` inserts a `reads` row (item, the digest it was shown in, now) and 302s to `canonical_url`; unknown id → 404. `POST /run` starts a manual run in the background: 202 (or 303 to `/` for a browser), 409 when a run is active, 403 when the request is cross-origin (`Sec-Fetch-Site` not `same-origin`/`none`, or `Origin` host ≠ `Host`), 429 past 3 manual runs in 24 h, 503 when runs are disabled. `GET /run/status` → `{ active }`. `/runs/{id}/transcript` streams the raw JSONL. Every response carries `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`. Every route except `/r/{id}`'s 404 sits behind the Access JWT middleware; the middleware is bypassed only when `CF_ACCESS_AUD` is unset **and** the bind is `127.0.0.1` (local dev), and a test proves `POST /run` is 401 without a valid assertion otherwise. Bind guard: the server refuses any bind other than `127.0.0.1`, or `0.0.0.0` with `DAILYBRIEF_IN_CONTAINER=1`.

Module ids for `/plan` slicing: `config-db` → `core-ingest` → `core-select` → `mcp-editor` → `editor-contract` → `harness-claude` → `web-read`; `seed` any time after `config-db`.

### 7c. Quality gates (`CONSTRAINTS.md` numbers)

| Dimension | Rule | Checked by | Runs at |
|---|---|---|---|
| Compile | zero warnings | `cargo build --all-targets` with `RUSTFLAGS="-D warnings"` | every edit |
| Lint / format | clippy clean at `-D warnings`; `cargo fmt --check` | `bin/dc cargo clippy --all-targets -- -D warnings && bin/dc cargo fmt --check` | every edit |
| Tests | all pass; no `#[ignore]` without a reason string | `bin/dc cargo test` (or `cargo nextest run`) | task end, CI |
| Coverage: core contracts | `core::`, `mcp::`, `db::`, `harness::` lines ≥ 80 % | `cargo llvm-cov --fail-under-lines 80` scoped by module | task end, CI |
| Coverage: rest | `web::`, `config::`, `commands::` ≥ 60 % | same | task end, CI |
| Generated schema | `schemas/digest.json` matches `editor::DigestOutput` | `cargo run --bin gen-schemas -- --check` | task end, CI |
| Dependencies | no advisories, allowed licences only | `cargo deny check` (`cargo audit` acceptable interim) | CI |
| Secrets in git history | zero findings | `gitleaks detect --redact` (install needs a decision — *ask first*; until then `git grep -n "sk-ant-\|ANTHROPIC_API_KEY"` at `/review`) | CI |
| Egress: spawn env | allowlist only; refuses API keys; requires OAuth token | `harness::claude_code` unit tests | task end, CI |
| Egress: transcript | no vault path, no topic description, no non-item text in a real transcript | `dailybrief scan-transcript <run>/transcript.jsonl` | manual gate before `/ship` |
| Bind | container publishes only `127.0.0.1:8788`; server refuses `0.0.0.0` unless `DAILYBRIEF_IN_CONTAINER=1` | `tests/compose.rs` + `web::app` test | task end, CI |
| Image hygiene | no secret in layers; `.env`, `data/`, `target/` in `.dockerignore`; `claude` pinned to an exact version | compose test + `docker history` in CI | CI |
| Check budget | `check:fast` (build + clippy + fmt) ≤ 60 s warm, `check:task` ≤ 4 min on the box, through `bin/dc` | wall clock in CI log | every run |

## 8. Build order — one agent skill + one Rust skill per milestone

| M | Deliverable | Skill practised (agent · Rust) | Gate |
|---|---|---|---|
| **R0** | Install agent-skills (§0); `/spec` + `/plan`; then **build M1 in full from this document**: config, migrations (adopt the ledger, §5), core functions, **rmcp server with the nine editor tools**, `claude-code` adapter with the §3 flags and env allowlist, runner (lock, attempts, wall clock, raw transcript), scheduler, `/`, `/d/{date}`, `/r/{id}`, `POST /run` behind Access JWT, `runs/{id}/transcript` download, CLI verbs incl. `reembed`, Docker image + compose. Expected ADRs: rmcp; dom_smoothie; fastembed + re-embed on first deployment; rusqlite; Access middleware pulled forward from M5 | tool-surface design, prompt-as-contract, headless harness configuration · ownership & borrowing in a real service, `serde`/`schemars` contract types, `tokio` process + blocking boundaries, `thiserror` error design | (a) **tests**: `cargo test` proves all nine tools through an in-process rmcp client, every `select` / `publish_digest` violation in §4, the 45-read cap, the third-rejection fatal flag, the adapter against a fake `claude` replaying recorded transcripts (success, `error_max_turns`, wall-clock kill, non-zero exit, no `result` line), retry-once, the env scrub, the bind guard, the byte-stable prompt and schema drift; (b) **live**: the container serves on port 8788 (§9), **three consecutive scheduled mornings** publish 24 + 6 on subscription usage, digest read on the phone, transcript scan clean; (c) rollback documented as one command |
| M2 | `run_events` ingestion from `stream-json`, `/runs/{id}` with per-turn tool calls, turns, tokens, wall time; turn/read/time caps surfaced; retry policy visible | trajectory capture and reading trajectories · `serde` internally-tagged enums with `#[serde(other)]` fallbacks for unknown events; streaming line parsing under backpressure | Every turn visible; a forced failure (`--max-turns 5`) shows correctly on `/` |
| M3 | Curator + `/curator` approval queue; ratings with reasons live (`ratings`, `proposals` migrations); a Claude Code `PreToolUse` hook as a second-layer policy on `propose_change` and on web-search count | approval gates, hooks, proposal-with-evidence · state machines as enums (`ProposalStatus`), transactional SQL in one `repo` call | First approved proposal changes next week's candidates measurably |
| M4 | **Codex adapter** on the same MCP server; alternate days for a week; compare turns, reads, explore quality, failures on `/runs` | harness portability · the `Harness` trait as the seam; `Box<dyn Harness>` vs generics decided by ADR | Side-by-side table on `/stats` |
| M5 | Ops hardening: `/stats`, `/feeds`, `/topics`; JWKS cache; structured logs reviewed; systemd user unit; runbook | ops · `tracing` spans across child process boundaries | Reachable only after OTP at dailybrief.hundredclouds.com; `/stats` shows reads/day |
| M6 | **Own-loop side quest** (`raw-loop` adapter, `reqwest`): `model: "claude-opus-5"`, `thinking: {"type": "adaptive"}`, `max_tokens` ≥ 16000, `tools` = the nine editor tools **as JSON Schema generated from the same `schemars` structs**, `cache_control: {"type": "ephemeral"}` on the system prompt and on the tool list; loop on `stop_reason == "tool_use"`, execute **all** `tool_use` blocks of a turn concurrently and return **all** `tool_result`s in **one** user message; check `stop_reason` for `max_tokens` and `refusal` (read `stop_details`) before running tools; parse tool inputs with `serde_json`, never string-match; context editing via beta header `context-management-2025-06-27` + `context_management: {"edits": [{"type": "clear_tool_uses_20250919"}]}`; verify `usage.cache_read_input_tokens` > 0 from turn 2. Run by hand on 5 candidates, never scheduled; budget ≤ $10 total; the key lives in a separate shell | stop reasons, `tool_use` ids, parallel results, caching, context management — the loop itself · hand-written HTTP client types, `futures::join_all`, retry with `tokio::time` backoff on 429/5xx | Cache-read ratio > 70 % from turn 5; you can explain every turn; `cargo test` covers the loop against a recorded-response fake |
| M7 | Evals: export 30 trajectories; programmatic graders (digest validity, reason present, no repeats, turn count) + LLM judge for explore-reason quality; regression set from failed runs | trajectory evals — feeds *Evals Flagship* · `insta` snapshot tests over trajectory summaries | Suite runs in CI on fixtures |
| M8 (optional) | Same Editor as a Claude Code **routine** (cloud) against the rmcp server over **streamable HTTP** (`transport-streamable-http-server`) through the tunnel with an Access service token | cloud scheduling, remote MCP, network policy · axum + rmcp sharing one router | One week side-by-side vs local headless |

R0 is deliberately the whole of M1: a partial build cannot be gated — the digest is the unit of correctness.

## 9. Deployment (Docker)

1. **Image**: multi-stage. Builder `rust:<pinned>-bookworm` → `cargo build --release` (cache `~/.cargo/registry` and `target/` with BuildKit mounts). Runtime `debian:bookworm-slim` + `ca-certificates` + **Claude Code** — prefer the **native installer binary** (`curl -fsSL https://claude.ai/install.sh | bash -s -- <exact version>`; *verify at build time* that it honours `CLAUDE_CODE_OAUTH_TOKEN` and `--restricted` identically to the npm build — fallback base: `node:26-bookworm-slim` with `npm i -g @anthropic-ai/claude-code@<exact version>`). Copy the `dailybrief` binary, `prompts/`, `schemas/`, `config/`. Non-root user uid 1000. `ENV DAILYBRIEF_IN_CONTAINER=1 DAILYBRIEF_CONFIG=/app/config/config.toml DAILYBRIEF_DATA_DIR=/data CLAUDE_CONFIG_DIR=/home/app/.claude DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1`. `EXPOSE 8788`; `CMD ["dailybrief", "serve"]`. `ort`'s bundled ONNX Runtime needs glibc — bookworm-slim is fine; **no Alpine/musl**.
2. **Compose**: profiles `app` (runtime image, `env_file: .env`, `ports: 127.0.0.1:8788:8788`, volumes `dailybrief-data:/data`, `dailybrief-claude:/home/app/.claude`, `init: true`, `restart: unless-stopped`), `dev` (bind-mounted source, `cargo watch -x 'run -- serve'`), `test` (bind-mounted source + a `cargo-target` named volume so host `target/` never mixes with the container's). `bin/dc cargo …` runs every check in the `test` container; **host cargo is never used for a gate**.
3. **First deployment**: stop whatever currently listens on 8788; copy the existing `brief.db` into the `dailybrief-data` volume; `dailybrief migrate` (a no-op on a current ledger); `dailybrief reembed`; `dailybrief fetch`; smoke run (`run --no-verify --attempts 1 --prompt prompts/smoke.md --schema schemas/smoke.json --message "Call get_briefing, then answer with the date."`), check the transcript's `system/init` lists `dailybrief` and exactly `WebSearch` + nine `mcp__dailybrief__*` tools; then `up -d`. **Rollback** = `docker compose --profile app down`, then `up -d` on the last good image tag; the DB file is copied into the volume, never bind-mounted, so a bad migration cannot damage the source copy.
4. **Token**: `claude setup-token` on the host, paste into `.env` as `CLAUDE_CODE_OAUTH_TOKEN`; record the one-year renewal date in the runbook. Never `ANTHROPIC_API_KEY`.
5. **Tunnel / Access**: unchanged — the existing `dailybrief.hundredclouds.com` public hostname → `http://localhost:8788` and the "Daily Brief" Access app; `CF_ACCESS_AUD` / `CF_ACCESS_TEAM` in `.env`.
6. **systemd** (user unit, no sudo): wraps `docker compose --profile app up` with `WorkingDirectory` = this repo.
7. Verify from a real client (server-side `curl` to the public name fails on this box's resolver; use `--resolve` locally).

## 10. Acceptance criteria (v4 done = R0 + M2–M5)

- A 30-item digest built by the Editor agent ships at 06:30 daily and on demand from the **Rust** service, on subscription usage; a failed run is shown as failed with its transcript, never silently substituted.
- The R0 gates passed and the service has run ≥ 14 days with no rollback.
- Every source click is recorded; `/stats` shows items read/day and the 7-day rate; `/runs/{id}` shows the full trajectory with per-turn tool calls, tokens and wall time.
- Ratings with reasons work; the weekly Curator produces proposals with evidence; nothing changes the profile without approval.
- The same rmcp server has been driven by two harnesses.
- Dead feeds and failed runs are visible on the page; reachable only via Cloudflare Access; only public article text and topic names leave the machine; `cargo clippy -D warnings`, `cargo deny` and the coverage floors are green in CI.
- 3-month review on **items read per day** (target ≥ 8 of 30), plus a practice review: which of R0–M7 you can now design and implement from memory — in Rust.

## 11. Constraints summary for `CONSTRAINTS.md` (hand this list to `constraint-driven-development`)

Subscription-only (no `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` anywhere in the service env, compose, Dockerfile, unit file; never `--bare`) · egress = public article text + topic names only, embeddings local, no vault read at runtime · no deterministic fallback · every non-digest state change through the approval queue · bind `127.0.0.1:8788` only · secrets never in the repo or image layers · same SQLite schema (DDL in §5), never re-authored · no `unsafe`, no `unwrap` outside tests/`main`, clippy `-D warnings` · every blocking call inside `spawn_blocking` · all SQL in `db::repo` · byte-stable editor prompt · every check runs in the `test` container · this file is never weakened to make a change pass.
