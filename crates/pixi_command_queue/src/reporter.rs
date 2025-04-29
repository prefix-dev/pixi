use pixi_git::resolver::RepositoryReference;

use crate::CondaEnvironmentSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SolveId(pub usize);

pub trait CondaSolveReporter {
    /// Called when the [`CommandQueue`] learns of a new environment to solve.
    ///
    /// The command_queue might not immediately start solving the environment,
    /// there is a limit on the number of active solves to avoid starving the
    /// CPU and memory.
    ///
    /// This function should return an identifier which is used to identify this
    /// particular solve. Other functions in this trait will use this identifier
    /// to link the events to the particular solve.
    fn on_solve_queued(&mut self, env: &CondaEnvironmentSpec) -> SolveId;

    /// Called when solving of the specified environment has started.
    fn on_solve_start(&mut self, solve_id: SolveId);

    /// Called when solving of the specified environment has finished.
    fn on_solve_finished(&mut self, solve_id: SolveId);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GitCheckoutId(pub usize);

pub trait GitCheckoutReporter {
    /// Called when a git checkout was queued on the [`CommandQueue`].
    fn on_git_checkout_queued(&mut self, env: &RepositoryReference) -> GitCheckoutId;

    /// Called when the git checkout has started.
    fn on_git_checkout_start(&mut self, checkout_id: GitCheckoutId);

    /// Called when the git checkout has finished.
    fn on_git_checkout_finished(&mut self, checkout_id: GitCheckoutId);
}

/// A trait that is used to report the progress of the [`CommandQueue`].
///
/// The reporter has to be `Send` but does not require `Sync`.
pub trait Reporter: CondaSolveReporter + GitCheckoutReporter + Send {}
