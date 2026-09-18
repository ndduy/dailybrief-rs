# ADR 0006: The MCP server is `rmcp`, tools are `#[tool]` methods over `core`

Date: 2026-09-18 · Status: accepted · Milestone: R0 (Task 13)

## Context
`SPEC.md` §3 makes an MCP server on stdio the one integration surface both harnesses speak. The Rust options were the official `rmcp` SDK (3.4, macros + schemars schemas + stdio and duplex transports) or a hand-written JSON-RPC loop.

## Decision
`rmcp` 3.4 with `server`, `transport-io`, `macros` and `schemars`; `client` only in tests. Tools are `#[tool]` methods on `mcp::server::DailyBriefServer` with doc comments as descriptions and `Parameters<T>` inputs (schemars structs, camelCase, `deny_unknown_fields`). Every tool body runs under `guarded`, which turns a `ToolError` or a panic into `CallToolResult::structured_error({"error": ...})` with `isError: true`; the only JSON-RPC errors are transport failures. Tests drive the real server through an in-process rmcp client over `tokio::io::duplex`, with both handshakes joined concurrently because the server side blocks until `initialize` arrives.

## Consequences
- `tools/list` (names, descriptions, input schemas) is an insta snapshot; changing a tool is a reviewed diff.
- Contract violations reach the model as feedback it can act on, never as a dead tool.
- `dailybrief mcp` must keep stdout for the protocol; logging goes to stderr in every command (tested by spawning the binary).
