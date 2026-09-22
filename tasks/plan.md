# Implementation Plan: M3 — ratings, the Curator, and the approval queue

Spec: `spec/m3.md` (approved 2026-09-22). Quality bar: `CONSTRAINTS.md`. Every check runs through `bin/dc`; deploys happen only between mornings with the running image tagged first; the migration deploy backs the database up first.

## Overview

Sixteen tasks in seven phases, one per capability-map module, foundation first. Phase 1 lands the second migration and the typed feedback model with the one transactional `apply`. Phase 2 puts rating buttons on the digest and lets positive ratings into the exploit profile. Phases 3 and 4 give the Curator its five tools, prompt, schema, role, hook and Sunday schedule. Phase 5 is the approval page, the only writer of topics and sources, with the "top-80 moves by ≥ 5" gate proven on a fixture. Phase 6 closes the M2 review backlog. Phase 7 is ADRs, the live migration, the first real Curator run and approval, and the ship.

## Architecture decisions (ADRs written at the task that decides them, `docs/adr/`)

- **0016 Ratings only record** (Task 2): the `ratings` table is evidence; the exploit scorer reads positive ratings as profile vectors next to reads (same window and rank weights); nothing else reads or writes on a rating. Supersedes `SPEC.md` §6's immediate effects.
- **0017 `seed` dropped** (Task 16): closes ADR 0010; `add_topic` proposals are the path for new topics.
- **0018 Migration `0002_feedback`** (Task 1): the DDL (`ratings`, `proposals`, `runs.role`), the rollback SQL, the backup step before the live apply, and the rule that the `m2` image must serve against a database that carries it.
- **0019 Hooks under `--restricted`** (Task 10, finalised at Task 16): whether `--settings` hooks fire on Claude Code 2.1.274 with `--restricted`; the hook policy is in the repo either way; the fallback layer is the server-side refusal plus the post-hoc scan.
- **Scheduler as a job list** (Task 11): `run_two_loops` becomes `run_jobs` over a list of `(Scheduler, job)` pairs with the same catch-up rule, so the third schedule does not add a fourth function.
- **Role-aware tool listing** (Task 5): one `DailyBriefServer` with a `Role`; rmcp's `ToolRouter` is composed from two `#[tool_router]` impls (`editor`, `curator`) and the server holds the one for its role (verified at the task).

## Dependency graph

```
Task 1  migration 0002 + rollback (feedback-db)
  └─ Task 2  core::feedback types, repo, transactional apply, writer guard test
       ├─ Task 3  rating buttons + POST /rate (rating-ui)
       │    └─ Task 4  positive ratings in the exploit profile
       ├─ Task 5  run role + role-aware tool listing (curator-tools)
       │    ├─ Task 6  get_feedback + get_profile
       │    ├─ Task 7  find_feeds + validate_feed
       │    └─ Task 8  propose_change
       │         └─ Task 9  curator prompt, schema, runner role, `curate` verb (curator-run)
       │              ├─ Task 10 PreToolUse hook + per-run settings
       │              └─ Task 11 scheduler job list + Sunday schedule
       │                   └─ Task 12 /curator approval page + apply + gate test (approval-web)
Task 13 security backlog (hardening-m3)        [independent]
Task 14 harness/core backlog                    [independent]
Task 15 deferred tests                          [independent]
Task 16 ADRs, live migration, first curate + approval, ship (ops-m3)   [after all]
```

## Vertical slices

| Phase | Slice that works at the end |
|---|---|
| 1 Feedback DB | `dailybrief migrate` applies 0002 on a copy of the live schema and its rollback restores 0001; a proposal round-trips and `apply` changes exactly its target in one transaction |
| 2 Rating UI | Two taps on the digest store a rating with a reason; a 👍 moves the item's rank in the exploit list |
| 3 Curator tools | An rmcp client sees the five curator tools on a curator server and none on an editor server; `propose_change` writes one row and nothing else |
| 4 Curator run | `dailybrief curate` against the fake writes proposals under `runs.role = curator`; the hook refuses a bad proposal and the ninth search; the loop fires the Curator on Sundays only |
| 5 Approval | `/curator` lists proposals with evidence; approve applies; the exploit top-80 moves by ≥ 5 on the fixture |
| 6 Hardening | Every M2 review item has its named test green |
| 7 Ship | 0002 applied live with a backup and a rollback rehearsal; one real Curator run; the first approval; Sunday recorded; ADRs 0016–0019 |

## Task list

### Phase 1: Feedback DB (`feedback-db`)
- [x] Task 1: Migration `0002_feedback` (ratings, proposals, `runs.role`) with rollback SQL and tests (ADR 0018)
- [ ] Task 2: `core::feedback` types, repo functions, transactional `apply`, the writer-guard test (ADR 0016)

### Checkpoint A
- [ ] `bin/check task` green; 0002 applies on a copy of `brief.db` and rolls back; the R0 ledger test still passes
- [ ] Review with human

### Phase 2: Rating UI (`rating-ui`)
- [ ] Task 3: Rating buttons with inline reasons on the digest card; `POST /rate` (store, replace, un-rate); 24 KiB cap
- [ ] Task 4: Positive ratings join the exploit profile as read-like vectors

### Checkpoint B
- [ ] `bin/check task` green; the rating flow proven by test; phone check after the Checkpoint E deploy
- [ ] Review with human

### Phase 3: Curator tools (`curator-tools`)
- [ ] Task 5: `Role` in the run, `${RUN_ROLE}` in `mcp.json`, role-aware tool listing with both listings snapshotted
- [ ] Task 6: `get_feedback` and `get_profile` (`core::curator_input`)
- [ ] Task 7: `find_feeds` (autodiscovery) and `validate_feed` (`core::feeds_discovery`)
- [ ] Task 8: `propose_change`: typed payloads, evidence, one row and nothing else

### Checkpoint C
- [ ] `bin/check task` green; editor `tools_list` snapshot unchanged; curator listing snapshotted
- [ ] Review with human

### Phase 4: Curator run (`curator-run`)
- [ ] Task 9: `prompts/curator.md`, `CuratorOutput` → `schemas/curator.json`, runner role, verification, `dailybrief curate`, fake fixture
- [ ] Task 10: `harness::hook` policy, `dailybrief hook`, per-run `settings.json` and `--settings` in argv, fake end-to-end (ADR 0019 draft)
- [ ] Task 11: Scheduler as a job list; `[curator] cron`; Sunday 07:30 in `serve`

### Checkpoint D
- [ ] `bin/check task` green; `dailybrief curate` against the fake writes proposals; the hook refuses in the fake run; Sundays only
- [ ] Review with human

### Phase 5: Approval (`approval-web`)
- [ ] Task 12: `/curator` page, approve/reject, `apply` as the only writer, day pages ignore curator runs, the top-80 gate test

### Checkpoint E
- [ ] `bin/check full` green; coverage not below M2
- [ ] Deploy between mornings: tag `m2`-running image, back up `brief.db`, migrate, rollback rehearsal on a copy with the `m2` image, watch the next 06:30 run
- [ ] Phone: rate one item; `/curator` opens (empty)
- [ ] Review with human

### Phase 6: Hardening (`hardening-m3`)
- [ ] Task 13: Security backlog — binary sha256 pin, log page event cap, escaping regression test, IPv6 transition ranges, `scan-transcript --run`
- [ ] Task 14: Harness/core backlog — accepted-select counting, `extracted` log after dedupe, fold once per page, bounded stdout line, orphaned `running` rows marked at start, head slot for the meta refresh
- [ ] Task 15: Deferred tests — JWKS interval, retry-chain window and kind, extract fall-through, wall from timestamps, boundaries, `prune --days` CLI, stale holder, barrier `fetch_sources`, heavier 64 KiB rows, cap test insert

### Checkpoint F
- [ ] `bin/check full` green; every `spec/m3.md` §9 #7 test name exists and passes
- [ ] Review with human

### Phase 7: Ops and ship (`ops-m3`)
- [ ] Task 16: ADRs 0016–0019 final; ADR 0014 result; TypeScript volumes removed (confirm first); runbook; one real `curate` (confirm first), first approval, next-morning measure; first Sunday recorded; R0 ship note closed; `CONSTRAINTS.md` measured; `docs/ship/m3.md`

### Checkpoint: Complete
- [ ] `spec/m3.md` §9 criteria 1–8 met with evidence
- [ ] `/review` (code-reviewer + security-auditor) and `/ship` run

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| The live migration fails or the `m2` image cannot start against a database with 0002 | High — a lost morning | Task 1 proves apply and rollback on a copy of `brief.db`; Checkpoint E backs up `brief.db` to a dated file, rehearses the `m2` image on a copy before the live apply, and deploys in the afternoon |
| Hooks in `--settings` do not fire under `--restricted` | Med — the hook is inert live | Task 10 verifies with the fake first, Task 16 with the real run; the server-side refusal and the post-hoc scan are the documented layer (ADR 0019) |
| rmcp cannot list a different tool set per server instance | Med — Phase 3 blocks | Task 5 starts with a 20-line spike (`ToolRouter` composition); fallback: one router, curator tools refuse with `isError` when the role is editor, and the listing test checks the refusal instead |
| htmx reason reveal needs inline script under the CSP | Low | attributes only (`hx-get` a partial, `hx-target`, `hx-swap`), verified in the browser at Checkpoint B |
| The digest page grows past 24 KiB with 30 rating widgets | Low | one shared form per card, reasons fetched on demand; the size test pins 24 KiB |
| The first real Curator run proposes nothing | Med — the approval gate has no input | the fixture-based tests are the gate for code; live, the ship note records the outcome; one retry with a nudged user message is pre-approved, more is ask-first |
| Positive-rating vectors swamp the profile | Low | same rank weights and window as reads; a test pins that one 👍 moves a rank but does not reorder the whole list |
| The scheduler refactor changes catch-up semantics | Low | the M2 tests move to the new API unchanged and must pass before the third schedule is added |

## Open questions

- Task 3: buttons under the summary (default) or in the title row (spec §13 #2).
- Task 8: evidence shape fixed fields plus notes (default, spec §13 #1).
- Task 6: explore hit-rate over the window (default) or per topic (spec §13 #3).
