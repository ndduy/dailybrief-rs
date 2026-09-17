//! Plain Rust core: fetching, extraction, dedupe, embeddings, scoring, staging (`SPEC.md` §3).

pub mod dedupe;
pub mod embed;
pub mod extract;
pub mod fetch;
pub mod http;
pub mod time;
pub mod vector;
