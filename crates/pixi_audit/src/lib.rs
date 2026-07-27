//! Audit locked packages for known vulnerabilities against an
//! OSV-compatible vulnerability API (basilisk).

pub mod client;
pub mod types;

pub use client::{AuditError, BASE_URL_ENV_VAR, BasiliskClient, DEFAULT_BASE_URL};
pub use types::*;
