use pixi_git::resolver::RepositoryReference;
use serde::Serialize;

use crate::{
    PixiEnvironmentSpec, SolveCondaEnvironmentSpec, install_pixi::InstallPixiEnvironmentSpec,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct PixiInstallId(pub usize);

pub trait PixiInstallReporter {
    /// Called when the [`crate::CommandDispatcher`] learns of a new pixi environment
    /// to install.
    ///
    /// This function should return an identifier which is used to identify this
    /// particular installation. Other functions in this trait will use this
    /// identifier to link the events to the particular solve.
    fn on_install_queued(
        &mut self,
        reason: Option<ReporterContext>,
        env: &InstallPixiEnvironmentSpec,
    ) -> PixiInstallId;

    /// Called when solving of the specified environment has started.
    fn on_install_start(&mut self, solve_id: PixiInstallId);

    /// Called when solving of the specified environment has finished.
    fn on_install_finished(&mut self, solve_id: PixiInstallId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct PixiSolveId(pub usize);

pub trait PixiSolveReporter {
    /// Called when the [`crate::CommandDispatcher`] learns of a new pixi environment
    /// to solve.
    ///
    /// The command_dispatcher might not immediately start solving the
    /// environment, there is a limit on the number of active solves to
    /// avoid starving the CPU and memory.
    ///
    /// This function should return an identifier which is used to identify this
    /// particular solve. Other functions in this trait will use this identifier
    /// to link the events to the particular solve.
    fn on_solve_queued(
        &mut self,
        reason: Option<ReporterContext>,
        env: &PixiEnvironmentSpec,
    ) -> PixiSolveId;

    /// Called when solving of the specified environment has started.
    fn on_solve_start(&mut self, solve_id: PixiSolveId);

    /// Called when solving of the specified environment has finished.
    fn on_solve_finished(&mut self, solve_id: PixiSolveId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct CondaSolveId(pub usize);

pub trait CondaSolveReporter {
    /// Called when the [`crate::CommandDispatcher`] learns of a new conda environment
    /// to solve.
    ///
    /// The command_dispatcher might not immediately start solving the
    /// environment, there is a limit on the number of active solves to
    /// avoid starving the CPU and memory.
    ///
    /// This function should return an identifier which is used to identify this
    /// particular solve. Other functions in this trait will use this identifier
    /// to link the events to the particular solve.
    fn on_solve_queued(
        &mut self,
        reason: Option<ReporterContext>,
        env: &SolveCondaEnvironmentSpec,
    ) -> CondaSolveId;

    /// Called when solving of the specified environment has started.
    fn on_solve_start(&mut self, solve_id: CondaSolveId);

    /// Called when solving of the specified environment has finished.
    fn on_solve_finished(&mut self, solve_id: CondaSolveId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct GitCheckoutId(pub usize);

pub trait GitCheckoutReporter {
    /// Called when a git checkout was queued on the [`crate::CommandDispatcher`].
    fn on_checkout_queued(
        &mut self,
        reason: Option<ReporterContext>,
        env: &RepositoryReference,
    ) -> GitCheckoutId;

    /// Called when the git checkout has started.
    fn on_checkout_start(&mut self, checkout_id: GitCheckoutId);

    /// Called when the git checkout has finished.
    fn on_checkout_finished(&mut self, checkout_id: GitCheckoutId);
}

/// A trait that is used to report the progress of the [`crate::CommandDispatcher`].
///
/// The reporter has to be `Send` but does not require `Sync`.
pub trait Reporter: Send {
    /// Returns a mutable reference to a reporter that reports on any git
    /// progress.
    fn as_git_reporter(&mut self) -> Option<&mut dyn GitCheckoutReporter>;
    /// Returns a mutable reference to a reporter that reports on conda solve
    /// progress.
    fn as_conda_solve_reporter(&mut self) -> Option<&mut dyn CondaSolveReporter>;
    /// Returns a mutable reference to a reporter that reports on an entire pixi
    /// solve progress. so that can mean solves for multiple ecosystems for
    /// an environment.
    fn as_pixi_solve_reporter(&mut self) -> Option<&mut dyn PixiSolveReporter>;
    /// Returns a mutable reference to a reporter that reports on the progress
    /// of actual package installation.
    fn as_pixi_install_reporter(&mut self) -> Option<&mut dyn PixiInstallReporter>;
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReporterContext {
    SolvePixi(PixiSolveId),
    SolveConda(CondaSolveId),
    InstallPixi(PixiInstallId),
}
