//! Forces uv to claim the global rayon pool before rattler's `Installer`
//! would race for it. Registered by default; UI reporters override the
//! slot and carry their own priming.

use std::sync::atomic::{AtomicU64, Ordering};

use pixi_build_discovery::JsonRpcBackendSpec;
use pixi_command_dispatcher::{
    InstallPixiEnvironmentSpec, InstantiateBackendReporter, PixiInstallReporter,
    PixiSolveEnvironmentSpec, PixiSolveReporter,
};
use pixi_compute_reporters::OperationId;
use uv_configuration::initialize_rayon_once;

#[derive(Default)]
pub struct RayonPrimer {
    next_id: AtomicU64,
}

impl RayonPrimer {
    fn alloc(&self) -> OperationId {
        OperationId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    fn prime() {
        initialize_rayon_once();
    }
}

impl PixiInstallReporter for RayonPrimer {
    fn on_queued(&self, _: &InstallPixiEnvironmentSpec) -> OperationId {
        Self::prime();
        self.alloc()
    }

    fn on_started(&self, _: OperationId) {}

    fn on_finished(&self, _: OperationId) {}
}

impl PixiSolveReporter for RayonPrimer {
    fn on_queued(&self, env: &PixiSolveEnvironmentSpec) -> OperationId {
        if env.has_direct_conda_dependency {
            Self::prime();
        }
        self.alloc()
    }

    fn on_started(&self, _: OperationId) {}

    fn on_finished(&self, _: OperationId) {}
}

impl InstantiateBackendReporter for RayonPrimer {
    fn on_queued(&self, _: &JsonRpcBackendSpec) -> OperationId {
        Self::prime();
        self.alloc()
    }

    fn on_started(&self, _: OperationId) {}

    fn on_finished(&self, _: OperationId) {}
}
