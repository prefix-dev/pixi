use std::collections::HashSet;

use itertools::Itertools;
use miette::Diagnostic;
use rattler_conda_types::{ParseChannelError, Platform};
use thiserror::Error;
use uv_distribution_filename::ExtensionError;

use crate::satisfiability::{ExcludeNewerMismatch, IndexesMismatch};

#[derive(Debug, Error, Diagnostic)]
pub enum EnvironmentUnsat {
    #[error("the channels in the lock-file do not match the environments channels")]
    ChannelsMismatch,

    #[error("platform(s) '{platforms}' present in the lock-file but not in the environment", platforms = .0.iter().map(|p| p.as_str()).join(", ")
    )]
    AdditionalPlatformsInLockFile(HashSet<Platform>),

    #[error(transparent)]
    IndexesMismatch(#[from] IndexesMismatch),

    #[error(transparent)]
    InvalidChannel(#[from] ParseChannelError),

    #[error(transparent)]
    InvalidDistExtensionInNoBuild(#[from] ExtensionError),

    #[error(
        "the lock-file contains non-binary package: '{0}', but the pypi-option `no-build` is set"
    )]
    NoBuildWithNonBinaryPackages(String),

    #[error(
        "the lock-file was solved with a different strategy ({locked_strategy}) than the one selected ({expected_strategy})",
        locked_strategy = fmt_solve_strategy(*.locked_strategy),
        expected_strategy = fmt_solve_strategy(*.expected_strategy),
    )]
    SolveStrategyMismatch {
        locked_strategy: rattler_solve::SolveStrategy,
        expected_strategy: rattler_solve::SolveStrategy,
    },

    #[error(
        "the lock-file was solved with a different channel priority ({locked_priority}) than the one selected ({expected_priority})",
        locked_priority = fmt_channel_priority(*.locked_priority),
        expected_priority = fmt_channel_priority(*.expected_priority),
    )]
    ChannelPriorityMismatch {
        locked_priority: rattler_solve::ChannelPriority,
        expected_priority: rattler_solve::ChannelPriority,
    },

    #[error(transparent)]
    ExcludeNewerMismatch(#[from] ExcludeNewerMismatch),
}

fn fmt_solve_strategy(strategy: rattler_solve::SolveStrategy) -> &'static str {
    match strategy {
        rattler_solve::SolveStrategy::Highest => "highest",
        rattler_solve::SolveStrategy::LowestVersion => "lowest-version",
        rattler_solve::SolveStrategy::LowestVersionDirect => "lowest-version-direct",
    }
}

fn fmt_channel_priority(priority: rattler_solve::ChannelPriority) -> &'static str {
    match priority {
        rattler_solve::ChannelPriority::Strict => "strict",
        rattler_solve::ChannelPriority::Disabled => "disabled",
    }
}
