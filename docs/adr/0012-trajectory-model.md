# ADR 0012: One typed `Event` over `stream-json`; turns are assistant message ids

Date: 2026-09-21 · Status: draft (Task 1; finalised at Task 2 and the M2 ship) · Milestone: M2

## Context
R0 stored every stdout line of `claude -p` in `run_events` and typed only `system/init` and
`result`. M2 needs turns, caps and tokens on a page, from the same stored lines, on a stream
whose vocabulary changes with Claude Code releases. `SPEC.md` §8 asks for internally tagged
`serde` enums with unknown-variant fallbacks.

## Decision
- `harness::trajectory::Event` is an internally tagged enum (`type`) with variants for every
  kind observed on Claude Code 2.1.274: `system` (all subtypes share one struct of optional
  fields; `init()` yields the runner's `InitEvent`), `assistant`, `user`, `tool_progress`,
  `rate_limit_event`, `result`.
- The fallback is `Event::Unknown { kind, raw }` produced by `Event::from_value`, **not**
  `#[serde(other)]` on a unit variant: the unit form would discard the value, and the page
  must be able to show an unknown line. A known `type` whose shape fails to parse is also
  `Unknown` with that kind, so a field renamed upstream degrades to "unknown" on the page
  rather than to a lost line. Content blocks do use `#[serde(other)] Other`.
- `claude_code::parse_stream_line` and `event_type` are views over `Event`; the strings stored
  in `run_events.type` are unchanged from R0.
- **A turn is one assistant `message.id`.** An assistant message arrives as several lines
  sharing its id (text, thinking, tool_use), each carrying the message's `usage` so far.

## Measured on the first real run (`2026-09-21-c8354589`, 337 lines)
- 107 assistant lines, 77 distinct message ids, all consecutive; 77 `tool_use` blocks and
  77 `tool_result` blocks; `result.num_turns` = **78**. `num_turns` counts one more than the
  message ids: the final round trip that produced the `result` after the `StructuredOutput`
  call. Task 2 asserts `num_turns - 1 <= turns <= num_turns`.
- Per-line `usage.output_tokens` do **not** sum to `result.usage.output_tokens` (1 890 over
  all lines, 1 695 over distinct ids, against 31 993 in the result, which also counts 17 439
  thinking tokens). Per-turn tokens are shown as the message reports them; the run's totals
  come from the `result` line only. Task 2's reconciliation is on turns and wall clock, not
  tokens.

## Consequences
- The fixture `tests/fixtures/transcripts/real-2026-09-21-c8354589.jsonl` (1 MB, public
  article text, `scan-transcript` clean) pins the vocabulary; a Claude Code bump re-runs
  `every_fixture_line_parses_to_a_known_variant` against a fresh capture.
- `/runs/{id}` (Task 4) supplements `/runs/{id}/log`; the raw page stays for the unknowns.
