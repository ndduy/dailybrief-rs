# ADR 0004: Coverage floors via one test run and two `llvm-cov report` invocations

Date: 2026-09-17 · Status: accepted · Milestone: R0

## Context
`CONSTRAINTS.md` sets two line-coverage floors: ≥ 80 % for `core::`, `mcp::`, `db::`, `harness::`, and ≥ 60 % for `web::`, `config::`, `commands::`. `cargo llvm-cov --fail-under-lines` applies one number to whatever files are in the report; it has no per-module floors. Running the suite twice to get two reports doubles the check budget.

## Decision
`bin/check full` runs the tests once with `cargo llvm-cov test --no-report`, then produces two reports from the same profile data:

```
cargo llvm-cov report --fail-under-lines 80 --ignore-filename-regex '^src/(web|config|commands)/|^src/main\.rs|^src/bin/'
cargo llvm-cov report --fail-under-lines 60 --ignore-filename-regex '^src/(core|mcp|db|harness)/|^src/main\.rs|^src/bin/'
```

`src/main.rs` and `src/bin/` are excluded from both (they are `anyhow` wiring). A third `report --json` is saved as a CI artifact for the "measured, not enforced" project-wide number.

## Consequences
- One test run; the floors are enforced per module group as the constraint reads.
- The regexes are the single place the module → floor mapping lives; a new top-level module must be added to one of them, and a CI grep fails if a `src/<mod>/` directory matches neither regex.
- *Verify at build time*: `cargo llvm-cov report` accepts `--fail-under-lines` and `--ignore-filename-regex` together in the installed version.
