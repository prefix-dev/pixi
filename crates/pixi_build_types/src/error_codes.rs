//! JSON-RPC error codes the build protocol assigns meaning to.
//!
//! JSON-RPC reserves `-32768..=-32000` for the protocol itself, so anything the
//! build protocol defines lives above that range.
//!
//! The distinction these encode is whose problem the failure is, which decides
//! what pixi tells the user: a backend that hit a bug is worth reporting to its
//! maintainers, while a recipe that does not parse is something the user can
//! fix themselves.

/// The backend failed for a reason of its own: a bug, a missing tool, an
/// unexpected state. The default for anything a backend does not classify.
pub const BACKEND_ERROR: i32 = -32000;

/// The user's recipe, manifest or configuration is at fault.
pub const USER_ERROR: i32 = -31000;
