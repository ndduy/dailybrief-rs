# Constraints

Last reviewed: 2026-09-17 by Duy (written during `/spec` R0 from `SPEC.md` §7c and §11; the numbers are the spec's, confirmed in the R0 spec interview).

Read this before writing code. Do not weaken it to make a change pass. Canonical source for every check; `bin/check` mirrors it.

## Floor (always enforced, no setup required)

- No `unsafe` (`#![forbid(unsafe_code)]` in `lib.rs`). No `unwrap()` / `expect()` outside `#[cfg(test)]` and `main.rs`.
- No new lint suppressions: `#[allow(...)]` needs a reason comment on the same line; no `#![allow(clippy::all)]` anywhere.
- No unimplemented stubs: `todo!()`, `unimplemented!()`, empty `Err(_) => {}` arms, `TODO` in shipped code paths.
- No skipped or deleted tests without the reason in the commit message; `#[ignore]` carries a reason string.
- No secrets in source: `ANTHROPIC_API_KEY`, `sk-ant-`, OAuth tokens, Cloudflare tokens — never in the repo, `.env.example`, fixtures or transcripts.
- No `--bare` and no `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` in any spawn environment, compose file, Dockerfile, or unit file (`SPEC.md` §2, §3). The only model credential is `CLAUDE_CODE_OAUTH_TOKEN` in the gitignored `.env`; it is never baked into an image.
- Only public article text and topic *names* leave the machine; embeddings are local; no vault content is read at runtime (`SPEC.md` §1 #7).
- No deterministic fallback for a failed run; no silent digest substitution (`SPEC.md` §1 #11).
- Every state change beyond today's digest goes through the approval queue; no R0 tool writes `topics` or `sources.weight` (`SPEC.md` §3).
- The SQLite schema is `SPEC.md` §5 verbatim; an applied migration is never edited; all SQL lives in `db::repo`.
- `rusqlite`, `fastembed` and `dom_smoothie` calls run inside `spawn_blocking`; nothing blocking runs on the async runtime.
- `prompts/editor.md` is byte-stable: no date, no id, no run-specific text.
- The `mcp` command writes nothing but JSON-RPC to stdout; every command logs to stderr.
- Every check runs inside the `test` container (`bin/dc …`) once the image exists; a green run on the host proves nothing for a gate.
- This file does not get weakened to make a change pass.

## Enforced with numbers

| Dimension | Rule | Checked by | Runs at | Reason |
|---|---|---|---|---|
| Compile | zero warnings | `cargo build --all-targets` with `RUSTFLAGS="-D warnings"` (`bin/check fast`) | every edit | a warning today is a bug in a month |
| Lint / format | clippy clean at `-D warnings`; `cargo fmt --check` clean | `cargo clippy --all-targets -- -D warnings && cargo fmt --check` (`bin/check fast`) | every edit | one style, no review noise; `clippy::pedantic` advisory at `/review` |
| Tests | all pass; no `#[ignore]` without a reason | `cargo test` (`bin/check task`) | task end, CI | toolkit rule: every task ships with tests |
| Coverage: core contracts | `core::`, `mcp::`, `db::`, `harness::`, `editor::` lines ≥ 80 % | `cargo llvm-cov` with the mechanism decided in ADR 0004 (`bin/check full`) | task end, CI | the agent depends on these invariants being real |
| Coverage: rest | `web::`, `config::`, `commands::` lines ≥ 60 % | same | task end, CI | views and wiring; integration tests cover the paths that matter |
| Generated schema | `schemas/digest.json` equals `editor::DigestOutput`'s schema, compared as parsed JSON | `cargo run --bin gen-schemas -- --check` (`bin/check task`) | task end, CI | the harness validates against the committed file |
| Dependencies | no advisories; licences only MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, MPL-2.0, CDLA-Permissive-2.0, OpenSSL (the last three as the resolved tree requires: compiler-support crates, Mozilla's CA bundle data, and the `ring` licence text; `deny_licences_match_constraints` keeps this list and `deny.toml` identical); no duplicate major versions of the same crate without a `deny.toml` reason | `cargo deny check` (`bin/check full`) | CI | supply-chain and licence hygiene |
| Secrets in git history | zero findings | `gitleaks detect --redact --no-banner` (`bin/check full`; installed on the box, approved 2026-09-17) | CI, before `/ship` | a token in history is a leaked token |
| Egress: spawn env | child env contains only `PATH HOME LANG TZ CLAUDE_CONFIG_DIR CLAUDE_CODE_OAUTH_TOKEN DISABLE_AUTOUPDATER CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC DAILYBRIEF_*`; refuses to start on `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN`; refuses without the OAuth token | `harness::claude_code` unit tests | task end, CI | the billing switch is silent otherwise |
| Egress: transcript | a real-run transcript contains no vault path, no topic description, no `read_item` text that is not the stored item text, ≤ 5 `WebSearch` calls | `dailybrief scan-transcript <run>/transcript.jsonl` (exit 0) | manual gate before `/ship` | `SPEC.md` §1 #7 |
| Bind | compose publishes only `127.0.0.1:8788`; the server refuses any bind other than `127.0.0.1`, or `0.0.0.0` with `DAILYBRIEF_IN_CONTAINER=1` | `tests/it/compose.rs` + `web::app` test | task end, CI | Cloudflare Access is the trust boundary |
| Auth | `POST /run` answers 401 without a valid `Cf-Access-Jwt-Assertion` whenever `CF_ACCESS_AUD` is set | `web::auth` test | task end, CI | a billed action must not be reachable unauthenticated |
| Image hygiene | no secret in layers; `.env`, `data/`, `target/` in `.dockerignore`; `claude` pinned to an exact version; non-root user | compose test + `docker history` in CI | CI | a token in a layer is a leaked token |
| Check budget | `bin/check fast` ≤ 60 s warm; `bin/check task` ≤ 4 min on the box, measured through `bin/dc` (container start included) | wall clock in the CI log | every run | slower checks get switched off; the ONNX model never loads in the fast loop |

## Measured, not yet enforced

| Metric | Today | Direction |
|---|---|---|
| Line coverage per group | core group (`core`, `mcp`, `db`, `harness`, `editor`) 95.61 %; rest group (`web`, `config`, `commands`) 82.58 % (2026-09-21, M2 Checkpoint D; R0 was 95.41 / 78.79) | must not fall |
| `bin/check fast` wall time | 0.8 s warm on the box (2026-09-17, host cargo, Task 25) | must stay ≤ 60 s |
| `bin/check task` wall time | 10–35 s warm through `bin/dc` on the box (2026-09-21, M2 tasks) | must stay ≤ 4 min |
| `bin/check full` wall time | 43 s warm through `bin/dc` on the box (2026-09-21, M2 Checkpoint D); 1496 s cold in CI (run 35303550178) | must stay ≤ 4 min on the box |
| Cold `cargo build --release` in the builder stage | 26 s with a warm BuildKit cache mount (2026-09-18); the cold builds were not timed cleanly (they failed on the network and on the glibc link, ADR 0011) | informs the workspace-split decision (`SPEC.md` §7: split only past ~20 s incremental) |
| Editor run turns / reads / wall time (from `runs`) | 84 / 33 / 10:17 (`2026-09-18-9f4cc78e`); 78 / 32 / 8:20 (`2026-09-21-c8354589`, the fixture); 93 turns / 10:37 (`2026-09-22-de8ebfdb`, first R0-gate morning); the run page shows these per run from M2 | recorded per run; M7 turns it into an eval |

## Exceptions

| ID | Rule | Path | Reason | Owner | Expires |
|---|---|---|---|---|---|
| X1 | Every check in the `test` container | whole repo | the Docker image lands in the `ops` module; host `cargo` 1.98.1 is acceptable for the slices before it | Duy | first R0 `/ship` |
| X2 | Coverage on the real-model embed path | `src/core/embed.rs` | exercised only by the opt-in `EMBED_REAL=1` test; the fake `Embedder` covers the call sites | Duy | M2 ship |
| X3 | No advisories (`RUSTSEC-2023-0071`, `rsa` Marvin timing attack) | `deny.toml` ignore | `rsa` arrives through `jsonwebtoken`'s pure-Rust backend and is used only to **verify** RS256 signatures with Cloudflare's public keys; the Marvin side channel concerns private-key operations, which happen only in tests (a key generated per run). No fixed `rsa` release exists. Revisit when `rsa` 0.10 ships or if the `aws_lc_rs` backend becomes acceptable in the image | Duy | 2026-12-17 |
| X4 | No advisories (`RUSTSEC-2024-0436`, `paste` unmaintained) | `deny.toml` ignore | a compile-time proc-macro reached through `tokenizers` ← `fastembed`; no runtime code, no known vulnerability. Revisit when the upstream dependency drops it | Duy | 2026-12-17 |
