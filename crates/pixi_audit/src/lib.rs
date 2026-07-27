//! Audit locked packages for known vulnerabilities against an
//! OSV-compatible vulnerability API (basilisk).

pub mod client;
pub mod report;
pub mod types;

pub use client::{AuditError, BASE_URL_ENV_VAR, BasiliskClient, DEFAULT_BASE_URL};
pub use report::{
    AuditPackage, AuditReport, AuditSummary, Finding, PackageEcosystem, SeverityBand,
    UncheckedPackage, audit,
};
pub use types::*;
