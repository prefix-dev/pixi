use pixi_manifest::PrioritizedChannel;
use rattler_conda_types::NamedChannelOrUrl;
use tokio::fs as tokio_fs;

use pixi_global::{EnvironmentName, project::SyncEnvironmentError};

/// An error from reverting an environment after a failed operation.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
enum RevertEnvironmentError {
    #[error("couldn't revert environment {env_name}")]
    Sync {
        env_name: EnvironmentName,
        #[source]
        source: Box<SyncEnvironmentError>,
    },

    #[error("failed to remove {}", .path.display())]
    RemoveDir {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// The channels an environment ends up with once it is set up: an
/// environment that already exists in the manifest keeps its channels
/// (unless `--force-reinstall` recreates it), a new one gets the
/// `--channel` arguments or the config's default channels. Name inference
/// runs before the manifest is touched and has to solve the build backend
/// against these same channels.
pub fn eventual_environment_channels(
    project: &pixi_global::Project,
    environment: Option<&EnvironmentName>,
    cli_channels: &[NamedChannelOrUrl],
    force_reinstall: bool,
) -> Vec<PrioritizedChannel> {
    if !force_reinstall
        && let Some(environment) = environment.and_then(|name| project.environment(name))
    {
        return environment.prioritized_channels().cloned().collect();
    }
    let channels = if cli_channels.is_empty() {
        project.config().default_channels()
    } else {
        cli_channels.to_vec()
    };
    channels.into_iter().map(PrioritizedChannel::from).collect()
}

/// Reverts the changes made to the project for a specific environment after an error occurred.
pub async fn revert_environment_after_error(
    env_name: &EnvironmentName,
    project_to_revert_to: &pixi_global::Project,
) {
    if let Err(err) = try_revert_environment(env_name, project_to_revert_to).await {
        tracing::warn!("Reverting of the operation failed");
        tracing::info!("Reversion error: {:?}", miette::Report::new(err));
    }
}

async fn try_revert_environment(
    env_name: &EnvironmentName,
    project_to_revert_to: &pixi_global::Project,
) -> Result<(), RevertEnvironmentError> {
    if project_to_revert_to.environment(env_name).is_some() {
        // We don't want to report on changes done by the reversion
        let _ = project_to_revert_to
            .sync_environment(env_name, None)
            .await
            .map_err(|source| RevertEnvironmentError::Sync {
                env_name: env_name.clone(),
                source: Box::new(source),
            })?;
    } else {
        // clean up if directory exists for the failed new environment
        let env_dir_path = project_to_revert_to.env_root_path().join(env_name.as_str());
        if env_dir_path.exists() {
            tokio_fs::remove_dir_all(&env_dir_path)
                .await
                .map_err(|source| RevertEnvironmentError::RemoveDir {
                    path: env_dir_path.clone(),
                    source,
                })?;
            tracing::debug!(
                "Cleaned up failed environment directory: {}",
                env_dir_path.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use tempfile::tempdir;

    use super::*;

    /// Create a project in an isolated `PIXI_HOME` so tests never read the
    /// global manifest of the machine they run on.
    async fn isolated_project(temp_dir: &std::path::Path) -> pixi_global::Project {
        let pixi_home_dir = temp_dir.join("pixi-home");
        temp_env::async_with_vars(
            [("PIXI_HOME", Some(pixi_home_dir.to_str().unwrap()))],
            pixi_global::Project::discover_or_create(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_eventual_environment_channels() {
        let temp_dir = tempdir().unwrap();
        let mut project = isolated_project(temp_dir.path()).await;

        let existing = EnvironmentName::from_str("existing").unwrap();
        let missing = EnvironmentName::from_str("missing").unwrap();
        let env_channel = NamedChannelOrUrl::from_str("env-channel").unwrap();
        let cli_channel = NamedChannelOrUrl::from_str("cli-channel").unwrap();
        project
            .manifest
            .add_environment(&existing, Some(vec![env_channel.clone()]))
            .unwrap();

        let defaults = project
            .config()
            .default_channels()
            .into_iter()
            .map(PrioritizedChannel::from)
            .collect::<Vec<_>>();

        // No target environment: --channel arguments or the defaults.
        assert_eq!(
            eventual_environment_channels(&project, None, &[], false),
            defaults
        );
        assert_eq!(
            eventual_environment_channels(
                &project,
                None,
                std::slice::from_ref(&cli_channel),
                false
            ),
            vec![PrioritizedChannel::from(cli_channel.clone())]
        );

        // A named environment that does not exist yet behaves the same.
        assert_eq!(
            eventual_environment_channels(
                &project,
                Some(&missing),
                std::slice::from_ref(&cli_channel),
                false
            ),
            vec![PrioritizedChannel::from(cli_channel.clone())]
        );

        // An existing environment keeps its manifest channels; --channel
        // arguments are not applied to it.
        assert_eq!(
            eventual_environment_channels(
                &project,
                Some(&existing),
                std::slice::from_ref(&cli_channel),
                false
            ),
            vec![PrioritizedChannel::from(env_channel)]
        );

        // --force-reinstall recreates the environment, so the manifest
        // channels no longer apply.
        assert_eq!(
            eventual_environment_channels(
                &project,
                Some(&existing),
                std::slice::from_ref(&cli_channel),
                true
            ),
            vec![PrioritizedChannel::from(cli_channel)]
        );
    }
}
