use std::num::NonZero;

/// Defines some limits for the command queue.
#[derive(Debug, Clone, Copy, Default)]
pub struct Limits {
    /// The maximum number of concurrent solves that can be performed. Solving
    /// conda environments can typically take up a lot of memory, so it is good
    /// practice to limit the number of concurrent solves.
    ///
    /// Typically, a good value is the number of CPU cores.
    pub max_concurrent_solves: Limit,
}

/// Defines the type of limit to apply.
#[derive(Debug, Clone, Copy, Default)]
pub enum Limit {
    /// There is no limit.
    None,

    /// There is an upper limit.
    Max(NonZero<usize>),

    /// Use a heuristic to determine the limit.
    #[default]
    Default,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolvedLimits {
    /// The maximum number of concurrent solves that can be performed.
    pub max_concurrent_solves: Option<usize>,
}

impl From<Limits> for ResolvedLimits {
    fn from(value: Limits) -> Self {
        let max_concurrent_solves = match value.max_concurrent_solves {
            Limit::None => None,
            Limit::Max(max) => Some(max.get()),
            Limit::Default => Some(
                std::thread::available_parallelism()
                    .map(NonZero::get)
                    .unwrap_or(1),
            ),
        };

        Self {
            max_concurrent_solves,
        }
    }
}
