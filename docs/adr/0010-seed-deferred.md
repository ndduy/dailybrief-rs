# ADR 0010: The `seed` command is deferred out of R0

Date: 2026-09-18 · Status: accepted · Milestone: R0 (spec interview, 2026-09-17)

## Context
`SPEC.md` §5 and §7b describe `dailybrief seed --vault <ro mount>` that drafts `topics.toml` from vault Areas. `config/topics.toml` is committed, hand-edited, and never re-seeded; the spec itself says the command is "not re-run".

## Decision
`seed` is not part of R0. It moves to M3 alongside the Curator, the next milestone that touches topics. R0 mirrors the committed `config/topics.toml` into the database before every run (`core::profile::sync_topics`) and that is the only path topics take into the system.

## Consequences
- No vault path is read by any R0 code; the egress scan's forbidden fragments include vault paths as a belt-and-braces check.
- The CLI has seven verbs in R0: `serve`, `run`, `fetch`, `mcp`, `migrate`, `reembed`, `scan-transcript`, plus the `gen-schemas` binary.
