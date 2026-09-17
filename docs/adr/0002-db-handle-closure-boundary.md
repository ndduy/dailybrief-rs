# ADR 0002: One `Db` handle owns the connection and the blocking boundary

Date: 2026-09-17 · Status: accepted · Milestone: R0

## Context
`SPEC.md` §7a: every `rusqlite` call runs inside `spawn_blocking`, all SQL lives in `db::repo`. If each repo function owns its own `spawn_blocking`, the async signature spreads through every caller and every function re-locks the connection; testing needs a runtime for trivial queries.

## Decision
- `db::Db` wraps `Arc<std::sync::Mutex<rusqlite::Connection>>` and exposes one async method: `call<T, F>(&self, f: F) -> Result<T, DbError>` where `F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static`. `call` does the `spawn_blocking`, the lock, and maps a poisoned lock or a cancelled task to `DbError`.
- `db::repo` functions are synchronous and take `&Connection` or `&mut Connection`. They are the only place SQL strings appear. Callers compose them inside one `Db::call` closure, which is also how a multi-statement transaction stays on one connection (`conn.transaction()` inside the closure).
- `Db::open(path)` sets, on every open: `journal_mode = WAL`, `synchronous = NORMAL`, `busy_timeout = 5000`, `foreign_keys = ON`, and runs the migration ledger. `Db::open_in_memory()` does the same for tests.
- One `Db` per process. The service process and the MCP child process each own one connection to the same file; WAL plus the busy timeout make the concurrent writes during a run safe.

## Consequences
- Repo functions are unit-tested synchronously against `Connection::open_in_memory()` with no runtime.
- Long closures block the single connection; the embedder is never called inside `Db::call` (embed first, then store).
- A test greps `src/` for `rusqlite::Connection` use outside `src/db/` and fails on any hit.
