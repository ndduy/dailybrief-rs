# ADR 0014: Keep or drop the `readability` fallback — decided by measurement

Date: 2026-09-21 · Status: proposed (Task 12 instruments; decided at the M3 /plan) · Milestone: M2

## Context
`core::extract` runs `dom_smoothie` first and falls back to the `readability` crate when the
first finds fewer than 20 words. The fallback pulls an older `html5ever` line (0.26 next to
0.39) onto the untrusted-HTML path, and nobody knows how often it rescues an article.

## Method
From Task 12 every stored item logs `extracted{feed, extractor, words}` with `extractor`
`dom_smoothie` or `readability`. The measurement is the count of each over the fetches
between the Checkpoint D deploy and the M2 ship:

```sh
docker compose --profile app logs --no-log-prefix app | grep '"message":"extracted"' \
  | grep -o '"extractor":"[a-z_]*"' | sort | uniq -c
```

## Decision rule
- Under 2 % of stored items from `readability` → drop the crate in M3's first task (and the
  duplicate `html5ever`); `extract` returns `None` where the fallback would have rescued.
- 2 % or more → keep it and record the duplicate in `deny.toml` with this ADR as the reason
  and a revisit date.

## Result
The instrumentation went live with the M2 deploy on 2026-09-22 07:35 local, so no measurement
existed at the M2 ship. The window runs from the 2026-09-23 morning fetch; the decision is
taken at the M3 `/plan` with the counts from the runbook command above (target: at least
14 mornings).
