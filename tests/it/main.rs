//! The one integration-test crate (`spec/r0.md` §5): every `mod` here compiles into a single
//! binary so the check budget stays flat as tests are added.

mod auth;
mod compose;
mod harness;
mod mcp;
mod mcp_stdout;
mod runner;
mod scenario;
mod web;
