use crate::CondaEnvironmentSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SolveId(pub usize);

trait CondaSolveReporter {
    /// Called when the [`Dispatcher`] learns of a new environment to solve.
    ///
    /// The dispatcher might not immediately start solving the environment,
    /// there is a limit on the number of active solves to avoid starving the
    /// CPU.
    ///
    /// This function should return an identifier which is used to identify this
    /// particular solve.
    fn on_solve_queued(&self, env: &CondaEnvironmentSpec) -> SolveId;

    /// Called when solving of the specified environment has started.
    fn on_solve_start(&self, solve_id: SolveId);

    /// Called when solving of the specified environment has finished.
    fn on_solve_finished(&self, solve_id: SolveId);
}
