# ADR 0019: PreToolUse hooks under `--restricted` (draft; finalised at M3 Task 16)

Date: 2026-09-22. Status: proposed (Task 10); the live result is recorded at Task 16.

## Context

`spec/m3.md` wants a second layer in front of the Curator's tools: a `PreToolUse` hook that
refuses a malformed `propose_change` and the ninth `WebSearch` before the call reaches the
model's tools. The harness runs `claude -p --restricted`, which ignores user, project and
local settings files, so the hook has to come from somewhere that mode still reads.

## Decision

- The hook policy is pure Rust (`harness::hook::decide`) and the `dailybrief hook pre-tool-use`
  verb wraps it with Claude Code's contract: the call JSON on stdin, exit 0 to allow, exit 2 to
  refuse with the reason on stderr. The web-search counter lives in `<run dir>/hook-state.json`.
- The runner renders `<run dir>/settings.json` per run (`hooks.PreToolUse`, matcher
  `mcp__dailybrief__propose_change|WebSearch`, command `'<this binary>' hook pre-tool-use`) and
  appends `--settings <path> --include-hook-events` to the argv. Claude Code 2.1.274's own help
  text for `--restricted` says "managed settings and `--settings` still apply", and
  `--include-hook-events` puts hook outcomes in the stream-json transcript, where the run page
  shows them.
- The fake `claude` has a `DAILYBRIEF_FAKE_RUN_HOOKS=1` mode that reads the rendered
  `settings.json` and runs the hook exactly as Claude Code does, writing a `{"type":"hook"}`
  line per decision; `fake_run_with_hooks_records_refusals` proves the end-to-end wiring.
- The hook is a second layer, never the only one: `propose_change` refuses the same inputs
  server-side, and `scan-transcript` counts `WebSearch` calls after the run.

## To confirm live (Task 16)

The first real `dailybrief curate` transcript must show hook events. If hooks do not fire
under `--restricted` on 2.1.274, the wiring stays (it is correct for the documented contract),
this ADR records the observed behaviour, and the server-side refusal plus the post-hoc scan
are the enforced layer (`spec/m3.md` §9 #5).
