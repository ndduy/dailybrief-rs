//! Plain Rust core: fetching, extraction, dedupe, embeddings, scoring, staging (`SPEC.md` §3).

pub mod candidates;
pub mod dedupe;
pub mod digest;
pub mod embed;
pub mod extract;
pub mod feedback;
pub mod fetch;
pub mod http;
pub mod ingest;
pub mod profile;
pub mod reembed;
pub mod retention;
pub mod staging;
#[cfg(test)]
pub mod testutil;
pub mod time;
pub mod vector;
