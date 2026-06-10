//! Workspace-side support for the PyPI resolve pipeline.
//!
//! The resolution itself lives in `pixi_install_pypi::resolve` and is driven
//! through the command dispatcher
//! ([`CommandDispatcher::solve_pypi_environment`](pixi_command_dispatcher::CommandDispatcher::solve_pypi_environment)).
//! This module provides the workspace implementation of
//! [`CondaPrefixProvider`]: it instantiates a conda prefix (and computes the
//! activated environment variables) on demand when a source distribution has
//! to be built during resolution.

use std::{cell::Cell, collections::HashMap, pin::Pin};

use pixi_install_pypi::resolve::{CondaPrefixProvider, ProvidedCondaPrefix};
use pixi_manifest::EnvironmentName;
use pixi_record::PixiRecord;

use crate::{
    activation::CurrentEnvVarBehavior,
    environment::CondaPrefixUpdater,
    workspace::{Environment, EnvironmentVars, get_activated_environment_variables},
};

/// Provides a conda prefix for PyPI source builds by installing the
/// environment's conda packages through a (memoized) [`CondaPrefixUpdater`]
/// and activating the environment.
pub struct WorkspaceCondaPrefixProvider<'p> {
    /// Instantiates (and memoizes) the conda prefix.
    prefix_updater: CondaPrefixUpdater,

    /// The conda records to install into the prefix. Consumed on first use;
    /// carries the error of the upstream solve, if any, so it surfaces only
    /// when a prefix is actually required.
    repodata_records: Cell<Option<miette::Result<Vec<PixiRecord>>>>,

    /// Project environment variables used to compute the activated
    /// environment of `environment`.
    project_env_vars: HashMap<EnvironmentName, EnvironmentVars>,
    environment: Environment<'p>,
}

impl<'p> WorkspaceCondaPrefixProvider<'p> {
    pub fn new(
        prefix_updater: CondaPrefixUpdater,
        repodata_records: miette::Result<Vec<PixiRecord>>,
        project_env_vars: HashMap<EnvironmentName, EnvironmentVars>,
        environment: Environment<'p>,
    ) -> Self {
        Self {
            prefix_updater,
            repodata_records: Cell::new(Some(repodata_records)),
            project_env_vars,
            environment,
        }
    }
}

impl CondaPrefixProvider for WorkspaceCondaPrefixProvider<'_> {
    fn provide(&self) -> Pin<Box<dyn Future<Output = miette::Result<ProvidedCondaPrefix>> + '_>> {
        Box::pin(async move {
            tracing::debug!(
                "PyPI solve requires instantiation of conda prefix for '{}'",
                self.prefix_updater.name().as_str()
            );

            let repodata_records = self
                .repodata_records
                .replace(None)
                .expect("the conda prefix can only be provided once")?;

            let prefix = self
                .prefix_updater
                .update(
                    repodata_records.into_iter().map(Into::into).collect(),
                    None,
                    None,
                )
                .await?;

            // Get the activation vars to expose to PEP 517 build backends.
            let env_vars = get_activated_environment_variables(
                &self.project_env_vars,
                &self.environment,
                CurrentEnvVarBehavior::Exclude,
                None,
                false,
                false,
            )
            .await?;

            Ok(ProvidedCondaPrefix {
                prefix: prefix.prefix.clone(),
                python_status: (*prefix.python_status).clone(),
                env_vars: env_vars.clone(),
            })
        })
    }
}
