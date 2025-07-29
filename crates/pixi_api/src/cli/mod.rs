use clap::Parser;
use miette::Diagnostic;
use pixi_consts::consts;

use thiserror::Error;

pub mod cli_config;
pub mod has_specs;

#[derive(Debug, Error, Diagnostic)]
pub enum LockFileUsageError {
    #[error("the argument '--locked' cannot be used together with '--frozen'")]
    FrozenAndLocked,
}

#[derive(Debug, Default, Copy, Clone)]
/// Lock file usage from the CLI with automatic validation
pub struct LockFileUsageArgs {
    inner: LockFileUsageArgsRaw,
}

#[derive(Parser, Debug, Default, Copy, Clone)]
#[group(multiple = false)]
/// Raw lock file usage arguments (use LockFileUsageArgs instead)
pub struct LockFileUsageArgsRaw {
    /// Install the environment as defined in the lockfile, doesn't update
    /// lockfile if it isn't up-to-date with the manifest file.
    #[clap(long, env = "PIXI_FROZEN", help_heading = consts::CLAP_UPDATE_OPTIONS)]
    pub frozen: bool,
    /// Check if lockfile is up-to-date before installing the environment,
    /// aborts when lockfile isn't up-to-date with the manifest file.
    #[clap(long, env = "PIXI_LOCKED", help_heading = consts::CLAP_UPDATE_OPTIONS)]
    pub locked: bool,
}

impl LockFileUsageArgs {
    pub fn frozen(&self) -> bool {
        self.inner.frozen
    }

    pub fn locked(&self) -> bool {
        self.inner.locked
    }
}

// Automatic validation when converting from raw args
impl TryFrom<LockFileUsageArgsRaw> for LockFileUsageArgs {
    type Error = LockFileUsageError;

    fn try_from(raw: LockFileUsageArgsRaw) -> Result<Self, LockFileUsageError> {
        if raw.frozen && raw.locked {
            return Err(LockFileUsageError::FrozenAndLocked);
        }
        Ok(LockFileUsageArgs { inner: raw })
    }
}

// For clap flattening - this provides automatic validation
impl clap::FromArgMatches for LockFileUsageArgs {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        let raw = LockFileUsageArgsRaw::from_arg_matches(matches)?;
        raw.try_into().map_err(|e: LockFileUsageError| {
            clap::Error::raw(clap::error::ErrorKind::ArgumentConflict, e.to_string())
        })
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

impl clap::Args for LockFileUsageArgs {
    fn augment_args(cmd: clap::Command) -> clap::Command {
        LockFileUsageArgsRaw::augment_args(cmd)
    }

    fn augment_args_for_update(cmd: clap::Command) -> clap::Command {
        LockFileUsageArgsRaw::augment_args_for_update(cmd)
    }
}

impl From<LockFileUsageArgs> for crate::environment::LockFileUsage {
    fn from(value: LockFileUsageArgs) -> Self {
        if value.frozen() {
            Self::Frozen
        } else if value.locked() {
            Self::Locked
        } else {
            Self::Update
        }
    }
}

impl TryFrom<LockFileUsageConfig> for crate::environment::LockFileUsage {
    type Error = LockFileUsageError;

    fn try_from(value: LockFileUsageConfig) -> Result<Self, LockFileUsageError> {
        value.validate()?;
        if value.frozen {
            Ok(Self::Frozen)
        } else if value.locked {
            Ok(Self::Locked)
        } else {
            Ok(Self::Update)
        }
    }
}

/// Configuration for lock file usage, used by LockFileUpdateConfig
#[derive(Parser, Debug, Default, Clone)]
pub struct LockFileUsageConfig {
    /// Install the environment as defined in the lockfile, doesn't update
    /// lockfile if it isn't up-to-date with the manifest file.
    #[clap(long, env = "PIXI_FROZEN", help_heading = consts::CLAP_UPDATE_OPTIONS)]
    pub frozen: bool,
    /// Check if lockfile is up-to-date before installing the environment,
    /// aborts when lockfile isn't up-to-date with the manifest file.
    #[clap(long, env = "PIXI_LOCKED", help_heading = consts::CLAP_UPDATE_OPTIONS)]
    pub locked: bool,
}

impl LockFileUsageConfig {
    /// Validate that the configuration is valid
    pub fn validate(&self) -> Result<(), LockFileUsageError> {
        if self.frozen && self.locked {
            return Err(LockFileUsageError::FrozenAndLocked);
        }
        Ok(())
    }
}
