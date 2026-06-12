//! Engine-side accessors for the shared resources of the PyPI pipeline.

use miette::miette;
use once_cell::sync::OnceCell;
use pixi_compute_engine::DataStore;
use pixi_uv_context::UvResolutionContext;

/// Lazily initialized [`UvResolutionContext`] registered in the engine's
/// data store.
///
/// Constructing the context forces the (otherwise lazy) reqwest client,
/// which loads the system certificate store — too expensive to do eagerly
/// on every engine construction when most operations never touch PyPI. The
/// initializer runs on first use and the result is memoized for the
/// engine's lifetime.
pub struct UvResolutionContextSource {
    cell: OnceCell<UvResolutionContext>,
    init: Box<dyn Fn() -> miette::Result<UvResolutionContext> + Send + Sync>,
}

impl UvResolutionContextSource {
    pub fn new(
        init: impl Fn() -> miette::Result<UvResolutionContext> + Send + Sync + 'static,
    ) -> Self {
        Self {
            cell: OnceCell::new(),
            init: Box::new(init),
        }
    }

    /// Returns the context, initializing it on first use.
    pub fn get(&self) -> miette::Result<&UvResolutionContext> {
        self.cell.get_or_try_init(|| (self.init)())
    }
}

/// Access the shared uv context (cache, concurrency, http settings) from
/// global data.
pub trait HasUvResolutionContext {
    fn uv_resolution_context(&self) -> miette::Result<&UvResolutionContext>;
}

impl HasUvResolutionContext for DataStore {
    fn uv_resolution_context(&self) -> miette::Result<&UvResolutionContext> {
        self.try_get::<UvResolutionContextSource>()
            .ok_or_else(|| {
                miette!(
                    "no `UvResolutionContextSource` was registered on the compute engine; \
                     register one through `CommandDispatcherBuilder::with_engine_data` before \
                     performing PyPI operations"
                )
            })?
            .get()
    }
}
