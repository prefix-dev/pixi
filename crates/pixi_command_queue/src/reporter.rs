use crate::CondaEnvironmentSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SolveId(pub usize);

pub trait CondaSolveReporter {
    /// Called when the [`CommandQueue`] learns of a new environment to solve.
    ///
    /// The dispatcher might not immediately start solving the environment,
    /// there is a limit on the number of active solves to avoid starving the
    /// CPU and memory.
    ///
    /// This function should return an identifier which is used to identify this
    /// particular solve. Other functions in this trait will use this identifier to
    /// link the events to the particular solve.
    fn on_solve_queued(&mut self, env: &CondaEnvironmentSpec) -> SolveId;

    /// Called when solving of the specified environment has started.
    fn on_solve_start(&mut self, solve_id: SolveId);

    /// Called when solving of the specified environment has finished.
    fn on_solve_finished(&mut self, solve_id: SolveId);
}


/// A trait that is used to report the progress of the [`CommandQueue`].
///
/// The reporter has to be `Send` but does not require `Sync`.
pub trait Reporter: CondaSolveReporter + Send {
    
}