# ADR 0013: Retention — 60 days of run directories and events, `runs` rows forever

Date: 2026-09-21 · Status: accepted · Milestone: M2 (Task 8)

## Context
Each run stores its transcript (about 1 MB for a full morning) under `/data/runs/<id>/` and
the same lines in `run_events`. R0 had no retention; `SPEC.md` names none. The run list on
`/runs` and the day pages are history the owner wants to keep.

## Decision
- `[retention] days = 60` (minimum 7) and `cron = "0 7 * * *"` in the service timezone.
- `dailybrief prune [--days N] [--dry-run]` and the daily job in `serve` (Task 9) call one
  function, `core::retention::prune`: finished runs started before the cutoff, never a run
  with `status = running`, never a `runs` row.
- Order per run: the directory (`transcript.jsonl`, `mcp.json`) first, then the `run_events`
  rows. A directory that cannot be removed stops the prune with the error and keeps its rows,
  so the pages never show a run whose file is gone but whose rows suggest otherwise; a
  directory that is already gone is fine.
- The job runs at 07:00, after the 06:30 run window and its wall clock, so a morning run is
  never pruned mid-flight; the `status = running` guard covers a manual run at that hour.

## Consequences
- `/runs` keeps every run since 2026-09-16 with status, turns and wall clock; older runs lose
  their turn table, log page and transcript download (the pages say so).
- Disk stays bounded at roughly 60 × the daily transcript size.
- The first live prune is an ask-first action (`spec/m2.md` §10): `--dry-run` first, list
  shown, then the real one.
