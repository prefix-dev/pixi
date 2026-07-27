//! Audit locked packages for known vulnerabilities against an
//! OSV-compatible vulnerability API (basilisk).

pub mod client;
pub mod report;

pub use client::{AuditError, BASE_URL_ENV_VAR, BasiliskClient, DEFAULT_BASE_URL};
pub use osv_protocol as types;
pub use osv_protocol::*;
pub use report::{
    AuditPackage, AuditReport, AuditSummary, Finding, PackageEcosystem, SeverityBand,
    UncheckedPackage, audit,
};
