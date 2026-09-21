# ADR 0015: Rollback is an image tag, not the TypeScript stack

Date: 2026-09-21 · Status: accepted · Milestone: M2 (Task 14)

## Context
R0's rollback was "start the TypeScript container again": the only other working service
at the time. From M2 on, every deploy has a previous Rust image, which is what `SPEC.md` §9
always meant by "`up -d` on the last good image tag". The TypeScript stack has diverged
(its own database, no run pages) and is removed at the M2 ship.

## Decision
- `compose.yaml` reads the app image from `DAILYBRIEF_IMAGE` (default `dailybrief-rs:runtime`).
- Rollback is one command: `docker compose --profile app down && DAILYBRIEF_IMAGE=dailybrief-rs:r0
  docker compose --profile app up -d`. Each milestone ship tags its image (`r0`, `m2`, …).
- **Tag before you build.** The M2 build moved the `runtime` tag off the image the live
  container was running, and the image store then dropped that image's content, so it could
  be neither tagged nor `docker commit`ted. `dailybrief-rs:r0` is therefore a rebuild of the
  deployed commit `a5f985c` from a git worktree (same base images, `--locked` build, pinned
  Claude Code 2.1.274): functionally the R0 ship, not the byte-identical object. The runbook's
  deploy procedure tags the running image first so this cannot recur.

## Consequences
- The TypeScript container `dailybrief-app-1` and its volumes are removed at the M2 ship
  (ask-first, `spec/m2.md` §10); the R0 rollback note in `docs/ship/r0.md` is superseded.
- A migration in a future milestone must state its own downgrade path in its ADR, because
  rolling the image back does not roll the schema back.
