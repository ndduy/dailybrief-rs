//! Harness adapters and the run lifecycle (`SPEC.md` §3): spawning `claude -p` on the
//! subscription, capturing `stream-json`, retrying once, and classifying the outcome.

pub mod claude_code;
pub mod runner;
pub mod scan_transcript;
pub mod scheduler;
pub mod service_runner;
pub mod trajectory;
pub mod types;
pub mod verify;
