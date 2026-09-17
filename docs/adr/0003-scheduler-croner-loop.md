# ADR 0003: Scheduler is a `croner` expression and a sleep loop

Date: 2026-09-17 · Status: accepted · Milestone: R0

## Context
`SPEC.md` §7 lists `tokio-cron-scheduler` 0.15 with a note to verify its timezone-aware constructor. R0 needs one cron line (`30 6 * * *` in `Asia/Ho_Chi_Minh`); M3 adds a weekly one. `tokio-cron-scheduler` brings a job store, uuid, and its own runtime plumbing for that.

## Decision
`harness::scheduler` parses `schedule.cron` with `croner` 4.0 and loops: compute the next occurrence from `Utc::now().with_timezone(&tz)` (`find_next_occurrence`, exclusive), `tokio::time::sleep` until it, then call the service runner with `RunKind::Scheduled` and log the outcome. The loop takes a `now: fn() -> DateTime<Utc>` and a `sleep` hook so tests drive it with a fake clock; the production `serve` command spawns it as one task. A run that overlaps the next tick is skipped by the run lock, and the loop logs the skip.

## Consequences
- ~50 lines, two dependencies fewer, no job store to reason about.
- The cron field validation in `config` uses `croner`'s parser, so an invalid expression fails at config load, not at 06:30.
- If M3's weekly job or M8 need more than two lines, revisit; the seam is `Scheduler::new(expr, tz, job)`.
- *Verify at build time*: `croner` 4 API for `Cron::new(..).parse()` and `find_next_occurrence(&DateTime<Tz>, inclusive: bool)`.
