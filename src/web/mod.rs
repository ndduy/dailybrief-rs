//! The reading surface (`SPEC.md` §7b "Web routes, precisely"): loopback only at the host boundary,
//! Cloudflare Access as the trust boundary, one digest page and its empty states.

pub mod app;
pub mod routes;
pub mod views;
