# ADR 0009: Cloudflare Access JWT verification ships in R0, not M5

Date: 2026-09-18 · Status: accepted · Milestone: R0 (Task 24)

## Context
`SPEC.md` §7 lists the Access JWT middleware under M5 but pulls it into R0 because `POST /run` is a billed action and must not be reachable unauthenticated, even behind the tunnel.

## Decision
`web::auth` verifies `Cf-Access-Jwt-Assertion` (RS256, audience `CF_ACCESS_AUD`, issuer the team URL, `exp` required) against the team JWKS cached for an hour and refetched once on an unknown `kid`; a stale cache serves through a failed refresh; 503 when there is no cache at all, 401 for every other failure. The middleware wraps every route. It is bypassed only when `CF_ACCESS_AUD` is unset **and** the bind is loopback; a non-loopback bind without it refuses to serve. In the container the bind is `0.0.0.0`, so the runtime image can never serve unauthenticated. `POST /run` adds a same-origin check, a rolling 24 h cap of three manual runs, and one live run per process.

## Consequences
- `.env` must carry `CF_ACCESS_AUD` and `CF_ACCESS_TEAM` for the app profile; compose passes them through, the process refuses to start without them.
- jsonwebtoken runs on its pure-Rust backend; the `rsa` advisory that brings is exception X3 in `CONSTRAINTS.md` (public-key verification only).
- Tests generate an RSA key per run; no key material lives in the repo.
