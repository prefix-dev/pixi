//! Reporting plumbing shared by pixi reporter implementations.
//!
//! Two complementary pieces:
//!
//! - [`OperationId`] / [`OperationRegistry`] / [`OperationIdSpawnHook`]:
//!   stable per-operation ids that thread parent links across compute
//!   spawns, so reporter callbacks know which higher-level work they
//!   belong to.
//! - [`ReporterLifecycle`] / [`LifecycleKind`] / [`Active`]: a tiny
//!   typestate wrapping the `on_queued -> on_started -> on_finished`
//!   sequence around any per-key reporter trait, with `on_finished`
//!   firing automatically on drop.

mod lifecycle;
mod operation_id;

pub use lifecycle::{Active, LifecycleKind, ReporterLifecycle, StartedReporterLifecycle};
pub use operation_id::{Ancestors, OperationId, OperationIdSpawnHook, OperationRegistry};
