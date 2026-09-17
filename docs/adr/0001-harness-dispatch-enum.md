# ADR 0001: Harness dispatch is an enum, not a trait object

Date: 2026-09-17 · Status: accepted · Milestone: R0 (`spec/r0.md` §2)

## Context
`SPEC.md` §3 names three harness adapters (`claude-code`, `codex` in M4, `raw-loop` in M6) behind one seam so the harness is a config value. A trait with `async fn run(&self, …)` is not object-safe without `async-trait` (a stated non-goal) or hand-rolled boxed futures. Generics (`Runner<H: Harness>`) work but push a type parameter through the runner, the service runner, the web state and the CLI.

## Decision
`harness::HarnessKind` is an enum with one variant per adapter (`ClaudeCode(ClaudeCodeAdapter)` in R0). It has an inherent `async fn run(&self, req, hooks) -> RunOutcome` that matches on `self`, and `fn name(&self) -> &'static str`. Runner, service runner and web state hold a `HarnessKind` by value. A `Harness` trait may still exist as documentation of the contract and for the fake adapter in tests, but nothing dispatches through `dyn Harness`.

## Consequences
- No `async-trait`, no `Pin<Box<dyn Future>>`, no generic parameter on the runner.
- Adding Codex in M4 is one variant and one match arm.
- Test fakes use a `HarnessKind::Fake(FakeAdapter)` variant compiled only under `cfg(test)`, or the fake `claude` executable on `PATH` (the R0 adapter tests use the latter).
