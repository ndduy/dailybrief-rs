# ADR 0011: Docker runtime on Debian trixie with the native Claude Code installer

Date: 2026-09-18 · Status: accepted · Milestone: R0 (Task 26)

## Context
`SPEC.md` §9 wants one image that carries the release binary and Claude Code at an exact
version, running as uid 1000 with only the `CLAUDE_CODE_OAUTH_TOKEN` from `.env` at run time.
Two choices were open: the Debian release under the binary, and how Claude Code gets into the
image (native installer versus npm on Node).

The first build on `bookworm` failed at link time: the prebuilt ONNX Runtime that `fastembed`
pulls in through `ort-download-binaries` references `__isoc23_strtol` and a `libstdc++`
`_M_replace_cold` symbol, i.e. it is built against glibc ≥ 2.38 and GCC 13+; bookworm ships
glibc 2.36 and GCC 12. Building ONNX Runtime from source or pinning an older `ort` binary were
the alternatives.

## Decision
- Builder, dev and runtime stages use **trixie** (`rust:1.98.1-trixie`, `debian:trixie-slim`,
  glibc 2.41). Never Alpine: `ort` needs glibc.
- Claude Code is installed by the **native installer** (`claude.ai/install.sh <version>`,
  checksum-verified) as the `app` user, pinned through the `CLAUDE_CODE_VERSION` build arg.
  No Node runtime in the image. The npm build stays the documented fallback: swap the installer
  line for `node:22-slim` + `npm i -g @anthropic-ai/claude-code@<version>` if a future version
  of the native build misbehaves under `-p --mcp-config`.
- Build steps run with `network: host` in `compose.yaml`: BuildKit's default build network
  could not resolve or reach the apt and rustup mirrors on this box (systemd-resolved stub on
  loopback), while running containers use the ordinary bridge.
- The `cargo-target` and `cargo-registry` volume mount points are created and chowned to `app`
  in the dev stage, because a named volume copies its mount point's owner on first use and the
  container runs as uid 1000.

## Verification (2026-09-18)
- `docker compose --profile app --profile test build`: both targets built; `dailybrief-rs:runtime`
  555 MB, `dailybrief-rs:dev` 3.1 GB.
- `docker run --rm dailybrief-rs:runtime id -u` → `1000`; `dailybrief --help` exits 0;
  `claude --version` → `2.1.274 (Claude Code)`; glibc in the image 2.41.
- `docker history --no-trunc dailybrief-rs:runtime` matches none of `sk-ant-`, `OAUTH_TOKEN=`,
  `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`; `env` in the container shows no key.
- `dailybrief migrate` then `dailybrief fetch` against a throw-away volume inside the runtime
  image: 36/36 feeds `ok`, the BGE model downloaded to `/data/models` (128 MB), `brief.db`
  12 MB with vectors. This is the ort + glibc proof.
- `bin/dc cargo test` inside the `test` container: 208 passed, 0 failed.
- `claude mcp list` in the image reports no user config, which is expected: the harness passes
  the rendered `mcp.json` per run with `--mcp-config` and never relies on the user config.

## Consequences
- CONSTRAINTS.md exception X1 (host `cargo` until the image lands) can close at the R0 ship;
  every check runs through `bin/dc` from here.
- The image tracks trixie's glibc; a future `ort` binary that needs something newer shows up
  as the same link error and is handled by bumping the base, not by patching symbols.
- Runtime image size is dominated by the ONNX Runtime and Claude Code, not by the binary.
