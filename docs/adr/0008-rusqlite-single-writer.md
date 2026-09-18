# ADR 0008: SQLite through `rusqlite` on one connection per process

Date: 2026-09-18 · Status: accepted · Milestone: R0 (Task 2)

## Context
`SPEC.md` §1 #14 fixes the schema and the file; §7 rejects `sqlx` (compile-time query checking needs a database at build time and an async pool buys nothing at ≤ 20k rows).

## Decision
`rusqlite` 0.40 with the bundled SQLite. One `Db` handle per process (ADR 0002) owns the connection, sets WAL, `synchronous=NORMAL`, `busy_timeout=5000`, `foreign_keys=ON` on every open, and applies the ledger. The DDL is the `SPEC.md` §5 text as a const; the ledger is `migrations(id, applied_at)` and `0001_init` is a no-op on the adopted database. Vectors are decoded with `as_chunks::<4>` and `f32::from_le_bytes`, never a pointer cast.

## Consequences
- Two processes (the service and the harness-spawned MCP server) write one file during a run; WAL plus the busy timeout make that safe (tested with 400 interleaved inserts from two handles).
- `db::repo` is the only place with SQL; a test greps the tree for `rusqlite` elsewhere.
