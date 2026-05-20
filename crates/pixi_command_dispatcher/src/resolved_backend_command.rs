//! Compute-engine Key that resolves a backend's [`JsonRpcBackendSpec`]
//! against the injected [`BackendOverrideKey`]
//! into a [`ResolvedBackendCommand`]: a concrete [`CommandSpec`] or an
//! in-memory instantiator factory. The Key does not call the
//! instantiator itself since `.initialize(..)` needs per-caller
//! source/workspace directories. Isolating the resolution behind a Key
//! lets upstream computes invalidate only when the backend they actually
//! use changes.

use std::{hash::Hash, sync::Arc};

use derive_more::Display;
use pixi_build_discovery::{CommandSpec, JsonRpcBackendSpec};
use pixi_build_frontend::{BackendOverride, in_memory::BoxedInMemoryBackend};
use pixi_compute_engine::{ComputeCtx, Key};

use crate::injected_config::BackendOverrideKey;

/// Key used to request a resolved backend command for an already
/// anchor-resolved [`JsonRpcBackendSpec`].
///
/// Wraps the spec in an [`Arc`] so dedup hits and subscribers clone
/// cheaply. Construct with [`ResolvedBackendCommandKey::new`].
#[derive(Clone, Debug, Hash, Eq, PartialEq, Display)]
#[display("{}", _0.name)]
pub struct ResolvedBackendCommandKey(pub Arc<JsonRpcBackendSpec>);

impl ResolvedBackendCommandKey {
    pub fn new(spec: JsonRpcBackendSpec) -> Self {
        Self(Arc::new(spec))
    }
}

/// The result of resolving a backend spec against the engine's
/// [`BackendOverrideKey`].
///
/// `Spec` carries an executable [`CommandSpec`] (possibly swapped out
/// by a System override). `InMemory` carries a factory that the caller
/// must drive with per-request [`pixi_build_types::procedures::initialize::InitializeParams`]
/// to get a live in-memory backend.
#[derive(Debug, Clone)]
pub enum ResolvedBackendCommand {
    Spec(CommandSpec),
    InMemory(BoxedInMemoryBackend),
}

impl Key for ResolvedBackendCommandKey {
    type Value = Arc<ResolvedBackendCommand>;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let override_value = ctx.compute(&BackendOverrideKey).await;
        let resolved = match override_value.as_ref() {
            BackendOverride::System(overrides) => ResolvedBackendCommand::Spec(
                overrides
                    .named_backend_override(&self.0.name)
                    .unwrap_or_else(|| self.0.command.clone()),
            ),
            BackendOverride::InMemory(overrides) => {
                match overrides.backend_override(&self.0.name) {
                    Some(in_mem) => ResolvedBackendCommand::InMemory(in_mem.clone()),
                    None => ResolvedBackendCommand::Spec(self.0.command.clone()),
                }
            }
        };
        Arc::new(resolved)
    }
}
