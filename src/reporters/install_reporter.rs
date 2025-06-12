use indicatif::MultiProgress;
use pixi_command_dispatcher::{
    InstallPixiEnvironmentSpec, ReporterContext, reporter::PixiInstallId,
};
use pixi_progress::ProgressBarPlacement;

use crate::reporters::main_progress_bar::MainProgressBar;

pub struct InstallReporter {
    sync_pb: MainProgressBar<String>,
}

impl InstallReporter {
    pub fn new(
        multi_progress: MultiProgress,
        progress_bar_placement: ProgressBarPlacement,
    ) -> Self {
        let sync_pb = MainProgressBar::new(
            multi_progress.clone(),
            progress_bar_placement,
            "syncing".to_owned(),
        );
        Self { sync_pb }
    }

    pub fn clear(&mut self) {
        self.sync_pb.clear();
    }
}

impl pixi_command_dispatcher::PixiInstallReporter for InstallReporter {
    fn on_queued(
        &mut self,
        _reason: Option<ReporterContext>,
        env: &InstallPixiEnvironmentSpec,
    ) -> PixiInstallId {
        let id = self.sync_pb.queued(env.name.clone());
        PixiInstallId(id)
    }

    fn on_start(&mut self, solve_id: PixiInstallId) {
        self.sync_pb.start(solve_id.0);
    }

    fn on_finished(&mut self, solve_id: PixiInstallId) {
        self.sync_pb.finish(solve_id.0);
    }
}
