# dailybrief-rs — AGENTS.md (Codex / generic harness entry)

Read `CLAUDE.md` (project rules, current milestone, fixtures, hard constraints) and `SPEC.md` (full specification) before any work. The mandated method is the addyosmani/agent-skills loop — skills are installed project-level under `.agents/skills/` (Codex) and `.claude/skills/` (Claude Code): spec → plan → build → test → review → ship, with evidence at every gate.

Codex is the **M4 harness** for running the Editor agent (see `SPEC.md` §3, §8); until M4 it is not required for development.
