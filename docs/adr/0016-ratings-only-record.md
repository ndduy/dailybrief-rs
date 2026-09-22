# ADR 0016: Ratings only record; every effect goes through a proposal

Date: 2026-09-22. Status: accepted (M3 Task 2). Supersedes the immediate effects in `SPEC.md` §6.

## Context

`SPEC.md` §6 lets a 👎 with `off_topic` lower a topic weight and a 👍 with `good_source` raise a
source weight at the moment of the tap. The M3 spec review chose against that: the reader's tap
is evidence, not a decision, and `SPEC.md` §1 #11 says nothing changes the profile without the
approval queue. A tap on a phone is also easy to mis-hit and hard to audit a week later.

## Decision

- The `ratings` table (migration `0002_feedback`, ADR 0018) is a record: `item_id`, optional
  `digest_id`, `sign`, one of eight fixed `reason`s, and `at`. One rating per item; a second tap
  replaces the first; un-rating deletes the row. Nothing else changes on a rating.
- Positive ratings feed the exploit profile the same way reads do (Task 4): the item's vector
  joins the profile with the same window and rank weights. That is scoring input, not state.
- Every other effect is a `proposals` row written by `core::feedback::propose` (the only thing
  the `propose_change` tool may do) and applied by `core::feedback::apply` from the approval
  page. `apply` runs in one transaction: the target row(s) and the proposal's own row change
  together or not at all, and a non-pending or missing-target proposal is refused unchanged.
- The five repo writers of topic and source state (`set_topic_weight`, `set_topic_origin`,
  `insert_topic_from_proposal`, `set_source_enabled`, `insert_source_from_proposal`) are called
  only from `core::feedback`; the integration test `only_feedback_apply_writes_topics_and_sources`
  greps `src/` for any other caller. `topics.toml` mirroring keeps using `upsert_topic`.
- Proposal payloads are typed (`ProposalChange`: `topic_weight`, `add_topic`, `disable_source`,
  `add_source`, `promote_explore_topic`) with bounds the prompt is told about: weight
  0.1..=5.0, name and title 1..=80 chars, description ≤ 200, notes ≤ 500, http(s) URLs only.
  An unknown kind never parses. New topic and source ids are slugs of the display name.

## Consequences

- A reader's ratings never surprise them: the digest looks the same the next morning until a
  proposal is approved. The Curator sees the ratings as evidence (`get_feedback`, Task 6).
- The approval page is a single point where the profile changes, with the before/after in
  `applied_json`, so a bad change is visible and reversible by a later proposal.
- `SPEC.md` §6's "immediate effects" paragraph is superseded; `spec/m3.md` §4 carries the rule.
