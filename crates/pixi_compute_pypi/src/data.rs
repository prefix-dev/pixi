//! Engine-side accessors for the shared resources of the PyPI pipeline.

use miette::miette;
use pixi_compute_engine::DataStore;
use pixi_uv_context::UvResolutionContext;

/// Access the shared uv context (cache, concurrency, http settings) from
/// global data.
pub trait HasUvResolutionContext {
    fn uv_resolution_context(&self) -> miette::Result<&UvResolutionContext>;
}

impl HasUvResolutionContext for DataStore {
    fn uv_resolution_context(&self) -> miette::Result<&UvResolutionContext> {
        self.try_get::<UvResolutionContext>().ok_or_else(|| {
            miette!(
                "no `UvResolutionContext` was registered on the compute engine; register one \
                 through `CommandDispatcherBuilder::with_engine_data` before performing PyPI \
                 operations"
            )
        })
    }
}
