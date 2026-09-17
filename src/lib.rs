//! dailybrief — personal daily-reading agent (library crate).
//!
//! `SPEC.md` is the source of truth; `spec/r0.md` is the current milestone.
#![forbid(unsafe_code)]

pub mod commands;
pub mod config;
pub mod core;
pub mod db;
pub mod mcp;
