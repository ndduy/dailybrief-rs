# Implementation Plan: M2 — trajectories on the page, and the R0 review backlog

Spec: `spec/m2.md` (approved 2026-09-21). Quality bar: `CONSTRAINTS.md`. Every check runs through `bin/dc`; deploys happen only between mornings (after 07:00, before 23:00 local).

## Overview

Fifteen tasks in seven phases, one per capability-map module, feature first. Phases 1 and 2 turn the stored `run_events` into a typed trajectory and put it on `/runs/{id}` with a caps bar and retry chain, and add the `--max-turns` override the gate needs. Phases 3 to 6 close the R0 review backlog by named test. Phase 7 is image and docs housekeeping, then the ship rehearsal: one real 5-turn run, the first live prune, the R0 image tagged as the rollback target.

## Architecture decisions (ADRs written at the task that decides them, `docs/adr/`)

- **0012 Trajectory model** (Task 1, refined at Task 2): one internally tagged `Event` enum with an `Unknown` fallback that keeps the raw value; a *turn* is one assistant `message.id` (its `text`, `thinking` and `tool_use` blocks arrive as separate lines sharing that id), and the fixture test pins `turn count == result.num_turns`; if they disagree the ADR records the definition that reconciles and why. `/runs/{id}` supplements `/runs/{id}/log`.
- **0013 Retention** (Task 8): 60 days by default, transcript file → run directory → `run_events` rows in that order, `runs` rows never; nothing younger than the window and nothing with `status = running` is touched; a daily job at 07:00 local after the run window.
- **0014 `readability` keep-or-drop** (Task 12 instruments, Task 15 decides): the ingest log records which extractor produced each item; the decision at `/ship` uses the counts since the instrumentation landed (under 2 % rescued → drop in M3's first task).
- **0015 Rollback by image tag** (Task 14): `dailybrief-rs:r0` is tagged from the running image before the first M2 deploy; rollback is `docker compose --profile app down` then `up -d` with `DAILYBRIEF_IMAGE=dailybrief-rs:r0`; the TypeScript container is removed at Task 15 after the user confirms.
- **Backpressure seam** (Task 3, folded into 0012): `Harness::run` takes an `mpsc::Sender<(u64, String)>` and awaits `send`, so the child blocks on its pipe when the writer is behind; the writer keeps the file write ahead of the database write, and a full channel is a log line, never a dropped line.

## Dependency graph

```
Task 1  Event enum + fixture (trajectory)
  └─ Task 2  fold_turns / caps_used / retry_chain
       ├─ Task 3  bounded-channel delivery (adapter seam)
       ├─ Task 4  /runs/{id} page, /runs columns, failed-state caps bar (run-view)
       │    └─ Task 5  --max-turns override + fake forced-failure rendering
       └─ Task 6  stderr redaction, runs.error scan, lossy stdout (harness-hardening)
            └─ Task 7  SIGKILL-after-grace + killed transcripts + fatal fourth publish
Task 8  core::retention + prune verb (retention)          [independent]
  └─ Task 9  daily prune job in serve
Task 10 web-hardening (status from lock, cap counts runs, JWKS throttle, HS256)   [independent]
Task 11 dedupe projection + title-hash window + cosine ingest test (core-hardening) [independent]
  └─ Task 12 fetch_sources idempotency + verify wording + extractor instrumentation
Task 13 vendored installer, purge curl/zstd (ops-m2)      [after 1–12: last image change]
  └─ Task 14 deny/CONSTRAINTS alignment, mcp_command removed, :r0 tag, runbook, ADR 0015
       └─ Task 15 ship rehearsal: gate run, first prune, TS container removal, ADRs final
```

## Vertical slices

| Phase | Slice that works at the end |
|---|---|
| 1 Trajectory | Every line of the real fixture parses to a typed event; turns, caps and the retry chain fold out of `run_events`; the adapter cannot drop a line |
| 2 Run view | `/runs/{id}` shows the caps bar, retry chain and every turn for the fixture; a fake `--max-turns 5` run renders "turns 5/5" on `/` |
| 3 Harness hardening | Stored error text is token-free; a hostile byte does not end a run; SIGKILL and `fatal` are proven |
| 4 Retention | `dailybrief prune --dry-run` lists what 60 days would remove; `serve` prunes daily after the morning run |
| 5 Web hardening | `/run/status` is true during a scheduled run; two retried runs do not exhaust the cap; JWKS refetch is throttled; HS256 confusion is rejected by test |
| 6 Core hardening | Ingest loads vectors once per batch; recurring titles come back after 14 days; concurrent `fetch_sources` fetches once; the extractor is logged |
| 7 Ops + ship | Image built from a vendored installer without `curl`; `:r0` tag exists; runbook written; the gate run, first prune and ADRs done |

## Task list

### Phase 1: Trajectory (`trajectory`)
- [x] Task 1: `harness::trajectory` — the `Event` enum, `parse_line`, the real-run fixture
- [x] Task 2: `fold_turns`, `caps_used`, `retry_chain`; fixture reconciliation
- [x] Task 3: Bounded-channel line delivery; lines never dropped under backpressure

### Checkpoint A
- [x] `bin/check task` green; every fixture line typed; turn count pinned to `num_turns - 1 ..= num_turns` (ADR 0012)
- [ ] Review with human

### Phase 2: Run view (`run-view`)
- [ ] Task 4: `/runs/{id}` page (caps bar, retry chain, turn table); `/runs` columns; failed-state caps bar
- [ ] Task 5: `dailybrief run --max-turns <n>`; forced-failure rendering against the fake

### Checkpoint B
- [ ] Phone-width check of `/runs/{id}` on a seeded temp DB (screenshot in the task note); the fake forced failure shows `turns 5/5` on `/`
- [ ] Review with human

### Phase 3: Harness hardening (`harness-hardening`)
- [ ] Task 6: Stderr redaction before storage; `scan-transcript` covers `runs.error`; lossy stdout lines
- [ ] Task 7: SIGKILL-after-grace and killed-run transcript tests; fourth publish after `fatal` refused

### Phase 4: Retention (`retention`)
- [ ] Task 8: `core::retention` and `dailybrief prune [--days] [--dry-run]` (ADR 0013)
- [ ] Task 9: Daily prune job in `serve` (`[retention]` config; 07:00 local)

### Checkpoint C
- [ ] `bin/check full` green; the retention tests and the four harness tests pass; coverage not below R0
- [ ] Review with human

### Phase 5: Web hardening (`web-hardening`)
- [ ] Task 10: `/run/status` from the lock holder; cap counts runs; JWKS refetch throttle; HS256 confusion test

### Phase 6: Core hardening (`core-hardening`)
- [ ] Task 11: `find_duplicate` on a per-batch `(id, vector)` projection; title-hash window; cosine near-duplicate ingest test
- [ ] Task 12: `fetch_sources` idempotent under concurrent calls; `verify` wording; extractor instrumentation (ADR 0014 measurement starts)

### Checkpoint D
- [ ] `bin/check full` green; every §9 #4 test name exists and passes except the three that belong to Tasks 13–15
- [ ] Deploy between mornings; next morning's run succeeds on the new image
- [ ] Review with human

### Phase 7: Ops and ship (`ops-m2`)
- [ ] Task 13: Vendored `install.sh` with sha256; `curl` and `zstd` purged from the runtime image
- [ ] Task 14: `deny.toml` ↔ `CONSTRAINTS.md`; drop `harness.mcp_command`; tag `dailybrief-rs:r0`; `docs/runbook.md`; ADR 0015
- [ ] Task 15: Ship rehearsal — one real `--max-turns 5` run (fixture captured), first live prune (dry-run first), TypeScript container removal (confirm first), ADRs 0012–0014 final, `CONSTRAINTS.md` measured

### Checkpoint: Complete
- [ ] `spec/m2.md` §9 criteria 1–6 met with evidence
- [ ] `/review` (code-reviewer + security-auditor) and `/ship` run

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| "Turn" as defined by grouping assistant lines does not equal `result.num_turns` (107 assistant lines, 78 turns in the real run) | Med — the caps bar would overstate turns | Task 1 measures distinct `message.id`s on the fixture before Task 2 is written; the reconciliation test is the acceptance, and ADR 0012 records the definition either way |
| Changing `Harness::run`'s hook to a channel touches the ADR 0001 seam and every adapter test | Med — a wide test diff | Task 3 keeps `HarnessKind::run` as the only public entry, changes the callback type once, and updates the fake-based tests mechanically; the argv snapshot is untouched |
| A bounded channel plus a slow `run_events` insert stalls the child mid-run | Low — the wall clock would kill a healthy run | The writer writes the file first and inserts second; channel capacity 1 024 lines; a full channel logs at warn; the 10 000-line test measures no stall beyond the writer's own pace |
| `prune` deletes the directory of a run that is still in flight or referenced by today's page | High — a failed page loses its transcript | Never prune `status = running`, never inside the window, never a `runs` row; `--dry-run` first on the box; the daily job runs at 07:00, after the run window |
| Purging `curl`/`zstd` breaks the installer step or the image | Low — the compose tests and `claude --version` in CI catch it | One `RUN` layer: install tools → run the vendored installer → `apt-get purge` → `claude --version`; CI's image checks stay |
| The 1 MB fixture bloats the repo | Low | Committed once, never edited; `.gitattributes` not used (removed by request); if it grows past one file, gzip in a later milestone |
| The gate run (`--max-turns 5`) leaves a failed run on today's page | Low — the day already has a digest by then | Run it after the morning digest exists; the page shows the digest, the run page shows the failure |
| Deploying mid-window collides with the 06:30 run | High — a morning lost | Boundary in `spec/m2.md` §10; Checkpoint D deploys in the afternoon and watches the next morning before Phase 7 |

## Open questions

- Task 4: does the digest success page get a run summary line or only a link? Default: link only (spec §13 #1).
- Task 9: prune at 07:00 local or 03:00? Default: 07:00 (spec §13 #2).
- Task 12: the extractor instrumentation is a log field, not a column (no migration in M2). If the measurement needs history, M3 adds a column with its own migration.
