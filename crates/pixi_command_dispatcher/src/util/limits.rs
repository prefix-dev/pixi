use std::num::NonZero;

/// Defines some limits for the command dispatcher.
#[derive(Debug, Clone, Copy, Default)]
pub struct Limits {
    /// The maximum number of concurrent solves that can be performed. Solving
    /// conda environments can typically take up a lot of memory, so it is good
    /// practice to limit the number of concurrent solves.
    ///
    /// Typically, a good value is the number of CPU cores.
    pub max_concurrent_solves: Limit,

    /// The maximum number of concurrent source builds that can be performed.
    /// Building takes up significant resources so we limit the total number of
    /// concurrent builds. By default only 1 build is allowed at a time.
    pub max_concurrent_builds: Limit,

    /// The maximum number of concurrent git checkouts. Git fetches are
    /// network-bound; too many in flight can overwhelm the network or
    /// remote server.
    pub max_concurrent_git_checkouts: Limit,

    /// The maximum number of concurrent URL archive fetches. Same
    /// rationale as `max_concurrent_git_checkouts`.
    pub max_concurrent_url_checkouts: Limit,

    /// The maximum number of concurrent filesystem operations performed while
    /// linking packages into prefixes. This bounds the file descriptors held
    /// during installation: the rattler installer opens many files at once,
    /// and installing several environments concurrently multiplies that, so a
    /// shared limit keeps the total from exhausting the file descriptor limit.
    pub max_io_concurrency: Limit,
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

impl From<usize> for Limit {
    fn from(value: usize) -> Self {
        NonZero::new(value).map(Limit::Max).unwrap_or(Limit::None)
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResolvedLimits {
    /// The maximum number of concurrent solves that can be performed.
    pub max_concurrent_solves: Option<usize>,

    /// The maximum number of concurrent source builds that can be performed.
    pub max_concurrent_builds: Option<usize>,

    /// The maximum number of concurrent git checkouts.
    pub max_concurrent_git_checkouts: Option<usize>,

    /// The maximum number of concurrent URL archive fetches.
    pub max_concurrent_url_checkouts: Option<usize>,

    /// The maximum number of concurrent filesystem operations during install.
    pub max_io_concurrency: Option<usize>,
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

        let max_concurrent_builds = match value.max_concurrent_builds {
            Limit::None => None,
            Limit::Max(max) => Some(max.get()),
            Limit::Default => Some(1), // Default to 1 build at a time
        };

        // Default to 8 concurrent network fetches: a common sweet spot
        // that's well below typical remote-host per-client connection
        // limits while still hiding per-request latency.
        let max_concurrent_git_checkouts = match value.max_concurrent_git_checkouts {
            Limit::None => None,
            Limit::Max(max) => Some(max.get()),
            Limit::Default => Some(8),
        };

        let max_concurrent_url_checkouts = match value.max_concurrent_url_checkouts {
            Limit::None => None,
            Limit::Max(max) => Some(max.get()),
            Limit::Default => Some(8),
        };

        // Default to 100 concurrent filesystem operations: this matches the
        // rattler installer's own default, but as a single semaphore shared
        // across all concurrent installs it no longer multiplies with the
        // number of environments being installed at once.
        let max_io_concurrency = match value.max_io_concurrency {
            Limit::None => None,
            Limit::Max(max) => Some(max.get()),
            Limit::Default => Some(100),
        };

        Self {
            max_concurrent_solves,
            max_concurrent_builds,
            max_concurrent_git_checkouts,
            max_concurrent_url_checkouts,
            max_io_concurrency,
        }
    }
}
