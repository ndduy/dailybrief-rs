# ADR 0005: `fastembed` on rustls, loaded lazily behind an `Embedder` trait

Date: 2026-09-17 · Status: accepted · Milestone: R0

## Context
`fastembed` 7.0's default features are `ort-download-binaries-native-tls`, `hf-hub-native-tls` and `image-models` (verified on crates.io 2026-09-17). The rest of the stack is rustls; two TLS stacks in one binary is avoidable. Loading the BGE-small model takes seconds and ~200 MB; the MCP process starts once per run and its first tool call is `get_briefing`, which needs no embeddings.

## Decision
- `Cargo.toml`: `fastembed = { version = "7", default-features = false, features = ["ort-download-binaries-rustls-tls", "hf-hub-rustls-tls"] }`. Model cache dir is `<data_dir>/models`; the first `fetch` downloads there.
- `core::embed::Embedder` is a trait: `fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>`, synchronous, `Send + Sync`. Callers invoke it inside `spawn_blocking`.
- `FastEmbedder` implements it and initialises the model on first use via `std::sync::OnceLock` (or `tokio::sync::OnceCell` if init must be awaited); construction is free. `FakeEmbedder` (hash → normalised unit vector, deterministic) implements it for tests; the real model is exercised only by the `EMBED_REAL=1` opt-in test.
- The re-embed decision from `SPEC.md` §7 stands: `dailybrief reembed` rewrites every item and topic vector once at first deployment; no compatibility check against existing vectors.

## Consequences
- Single TLS stack; `cargo deny` sees one `ring`/`rustls` lineage.
- `get_briefing` and the web server never pay the model load.
- *Verify at build time*: exact feature names on docs.rs for the resolved 7.x, `TextInitOptions::with_cache_dir`, and whether `TextEmbedding` is `Send + Sync` (else wrap in a `Mutex` inside `FastEmbedder`).
