use std::path::PathBuf;

use clap::Parser;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pep508_rs::ExtraName;
use pixi_core::{WorkspaceLocator, workspace::Environment};
use pixi_manifest::{FeaturesExt, pypi::pypi_options::FindLinksUrlOrPath};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName, VersionOrStar};
use rattler_conda_types::{
    ChannelConfig, EnvironmentYaml, MatchSpec, MatchSpecOrSubSection, NamedChannelOrUrl,
    ParseStrictness, Platform,
};

use crate::cli_config::WorkspaceConfig;

#[derive(Debug, Parser)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// Explicit path to export the environment file to.
    pub output_path: Option<PathBuf>,

    /// The platform to render the environment file for.
    /// Defaults to the current platform.
    #[arg(short, long)]
    pub platform: Option<Platform>,

    /// The environment to render the environment file for.
    /// Defaults to the default environment.
    #[arg(short, long)]
    pub environment: Option<String>,
}

fn format_pip_extras(extras: &[ExtraName]) -> String {
    if extras.is_empty() {
        return String::new();
    }
    format!(
        "[{}]",
        extras.iter().map(|extra| format!("{extra}")).join("")
    )
}

fn format_pip_dependency(name: &PypiPackageName, requirement: &PixiPypiSpec) -> String {
    match requirement {
        PixiPypiSpec::Git {
            url: git_url,
            extras,
        } => {
            let mut git_string = format!(
                "{name}{extras} @ git+{url}",
                name = name.as_normalized(),
                extras = format_pip_extras(extras),
                url = git_url.git,
            );

            if let Some(Some(rev)) = git_url.rev.as_ref().map(|rev| rev.reference()) {
                git_string.push_str(&format!("@{rev}"));
            }

            if let Some(ref subdirectory) = git_url.subdirectory {
                git_string.push_str(&format!("#subdirectory=={subdirectory}"));
            }

            git_string
        }
        PixiPypiSpec::Path {
            path,
            editable,
            extras,
        } => {
            if let Some(_editable) = editable {
                format!(
                    "-e {path}{extras}",
                    path = path.to_string_lossy(),
                    extras = format_pip_extras(extras),
                )
            } else {
                format!(
                    "{path}{extras}",
                    path = path.to_string_lossy(),
                    extras = format_pip_extras(extras),
                )
            }
        }
        PixiPypiSpec::Url {
            url,
            subdirectory,
            extras,
        } => {
            let mut url_string = format!(
                "{name}{extras} @ {url}",
                name = name.as_normalized(),
                extras = format_pip_extras(extras),
                url = url,
            );

            if let Some(subdirectory) = subdirectory {
                url_string.push_str(&format!("#subdirectory=={subdirectory}"));
            }

            url_string
        }
        PixiPypiSpec::Version {
            version, extras, ..
        } => {
            format!(
                "{name}{extras}{version}",
                name = name.as_normalized(),
                extras = format_pip_extras(extras),
                version = version
            )
        }
        PixiPypiSpec::RawVersion(version) => match version {
            VersionOrStar::Version(_) => format!(
                "{name}{version}",
                name = name.as_normalized(),
                version = version
            ),
            VersionOrStar::Star => format!("{name}", name = name.as_normalized()),
        },
    }
}

fn build_env_yaml(
    platform: &Platform,
    environment: &Environment,
    config: &ChannelConfig,
) -> miette::Result<EnvironmentYaml> {
    let channels =
        channels_with_nodefaults(environment.channels().into_iter().cloned().collect_vec());
    let mut env_yaml = rattler_conda_types::EnvironmentYaml {
        name: Some(environment.name().as_str().to_string()),
        channels,
        ..Default::default()
    };

    let mut pip_dependencies: Vec<String> = Vec::new();

    for (name, pixi_spec) in environment
        .combined_dependencies(Some(*platform))
        .into_specs()
    {
        if let Some(nameless_spec) = pixi_spec
            .clone()
            .try_into_nameless_match_spec(config)
            .into_diagnostic()?
        {
            let spec = MatchSpec::from_nameless(nameless_spec, Some(name.clone()));
            env_yaml
                .dependencies
                .push(MatchSpecOrSubSection::MatchSpec(Box::new(spec)));
        } else {
            tracing::warn!(
                "Failed to convert dependency to conda environment spec: {:?}. Skipping dependency",
                name
            );
        }
    }

    if environment.has_pypi_dependencies() {
        for (name, requirement) in environment.pypi_dependencies(Some(*platform)).into_specs() {
            pip_dependencies.push(format_pip_dependency(&name, &requirement));
        }
    }

    if !pip_dependencies.is_empty() {
        let pypi_options = environment.pypi_options();
        if let Some(ref find_links) = pypi_options.find_links {
            for find_link in find_links {
                match find_link {
                    FindLinksUrlOrPath::Url(url) => {
                        pip_dependencies.insert(0, format!("--find-links {url}"));
                    }
                    FindLinksUrlOrPath::Path(path) => {
                        pip_dependencies
                            .insert(0, format!("--find-links {}", path.to_string_lossy()));
                    }
                }
            }
        }
        if let Some(ref extra_index_urls) = pypi_options.extra_index_urls {
            for extra_index_url in extra_index_urls {
                pip_dependencies.insert(0, format!("--extra-index-url {extra_index_url}"));
            }
        }
        if let Some(ref index_url) = pypi_options.index_url {
            pip_dependencies.insert(0, format!("--index-url {index_url}"));
        }

        env_yaml
            .dependencies
            .push(MatchSpecOrSubSection::MatchSpec(Box::new(
                MatchSpec::from_str("pip", ParseStrictness::Lenient)
                    .expect("'pip' should be a valid name"),
            )));

        env_yaml
            .dependencies
            .push(MatchSpecOrSubSection::SubSection(
                "pip".to_string(),
                pip_dependencies.into_iter().collect_vec(),
            ));
    }

    Ok(env_yaml)
}

/// Add `nodefaults` channel if the environment doesn't have `main`, `r`, or
/// `msys2`
fn channels_with_nodefaults(channels: Vec<NamedChannelOrUrl>) -> Vec<NamedChannelOrUrl> {
    let mut channels = channels;
    if !channels.iter().any(|channel| {
        let channel = channel.as_str().to_lowercase();
        channel == "main" || channel == "r" || channel == "msys2"
    }) {
        channels.push(NamedChannelOrUrl::Name("nodefaults".to_string()));
    }
    channels
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;
    let environment = workspace.environment_from_name_or_env_var(args.environment)?;
    let platform = args.platform.unwrap_or_else(|| environment.best_platform());
    let config = workspace.config();

    let env_yaml = build_env_yaml(&platform, &environment, config.global_channel_config())?;

    if let Some(output_path) = args.output_path {
        env_yaml
            .to_path(output_path.as_path())
            .into_diagnostic()
            .with_context(|| "failed to write environment YAML")?;
    } else {
        println!("{}", env_yaml.to_yaml_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_core::Workspace;
    use std::path::Path;

    #[test]
    fn test_export_conda_env_yaml() {
        let path = Path::new(env!("CARGO_WORKSPACE_DIR"))
            .join("tests/data/mock-projects/test-project-export/pixi.toml");
        let workspace = Workspace::from_path(&path).unwrap();
        let args = Args {
            output_path: None,
            platform: Some(Platform::Osx64),
            environment: Some("default".to_string()),
            workspace_config: WorkspaceConfig::default(),
        };
        let environment = workspace
            .environment_from_name_or_env_var(args.environment)
            .unwrap();
        let platform = args.platform.unwrap_or_else(|| environment.best_platform());

        let env_yaml = build_env_yaml(
            &platform,
            &environment,
            workspace.config().global_channel_config(),
        );
        insta::assert_snapshot!(
            "test_export_conda_env_yaml",
            env_yaml.unwrap().to_yaml_string()
        );
    }

    #[test]
    fn test_export_conda_env_yaml_with_pip_extras() {
        let path = Path::new(env!("CARGO_WORKSPACE_DIR")).join("examples/pypi/pixi.toml");
        let workspace = Workspace::from_path(&path).unwrap();
        let args = Args {
            output_path: None,
            platform: None,
            environment: Some("default".to_string()),
            workspace_config: WorkspaceConfig::default(),
        };
        let environment = workspace
            .environment_from_name_or_env_var(args.environment)
            .unwrap();
        let platform = args.platform.unwrap_or_else(|| environment.best_platform());

        let env_yaml = build_env_yaml(
            &platform,
            &environment,
            workspace.config().global_channel_config(),
        );
        insta::assert_snapshot!(
            "test_export_conda_env_yaml_with_pip_extras",
            env_yaml.unwrap().to_yaml_string()
        );
    }

    #[test]
    fn test_export_conda_env_yaml_with_pip_source_editable() {
        let path =
            Path::new(env!("CARGO_WORKSPACE_DIR")).join("examples/pypi-source-deps/pixi.toml");
        let workspace = Workspace::from_path(&path).unwrap();
        let args = Args {
            output_path: None,
            platform: None,
            environment: Some("default".to_string()),
            workspace_config: WorkspaceConfig::default(),
        };
        let environment = workspace
            .environment_from_name_or_env_var(args.environment)
            .unwrap();
        let platform = args.platform.unwrap_or_else(|| environment.best_platform());

        let env_yaml = build_env_yaml(
            &platform,
            &environment,
            workspace.config().global_channel_config(),
        );
        insta::assert_snapshot!(
            "test_export_conda_env_yaml_with_source_editable",
            env_yaml.unwrap().to_yaml_string()
        );
    }

    #[test]
    fn test_export_conda_env_yaml_with_pip_custom_registry() {
        let path =
            Path::new(env!("CARGO_WORKSPACE_DIR")).join("examples/pypi-custom-registry/pixi.toml");
        let workspace = match Workspace::from_path(&path) {
            Ok(workspace) => workspace,
            Err(err) => {
                panic!("Failed to load workspace: {:?}", err);
            }
        };
        let args = Args {
            output_path: None,
            platform: None,
            environment: Some("alternative".to_string()),
            workspace_config: WorkspaceConfig::default(),
        };
        let environment = workspace
            .environment_from_name_or_env_var(args.environment)
            .unwrap();
        let platform = args.platform.unwrap_or_else(|| environment.best_platform());

        let env_yaml = build_env_yaml(
            &platform,
            &environment,
            workspace.config().global_channel_config(),
        );
        insta::assert_snapshot!(
            "test_export_conda_env_yaml_with_pip_custom_registry",
            env_yaml.unwrap().to_yaml_string()
        );
    }

    #[test]
    fn test_export_conda_env_yaml_with_pip_find_links() {
        let path =
            Path::new(env!("CARGO_WORKSPACE_DIR")).join("examples/pypi-find-links/pixi.toml");
        let workspace = Workspace::from_path(&path).unwrap();
        let args = Args {
            output_path: None,
            platform: None,
            environment: Some("default".to_string()),
            workspace_config: WorkspaceConfig::default(),
        };
        let environment = workspace
            .environment_from_name_or_env_var(args.environment)
            .unwrap();
        let platform = args.platform.unwrap_or_else(|| environment.best_platform());

        let env_yaml = build_env_yaml(
            &platform,
            &environment,
            workspace.config().global_channel_config(),
        );
        insta::assert_snapshot!(
            "test_export_conda_env_yaml_with_pip_find_links",
            env_yaml.unwrap().to_yaml_string()
        );
    }

    #[test]
    fn test_export_conda_env_yaml_pyproject_panic() {
        let path = Path::new(env!("CARGO_WORKSPACE_DIR")).join("examples/docker/pyproject.toml");
        let workspace = Workspace::from_path(&path).unwrap();
        let args = Args {
            output_path: None,
            platform: Some(Platform::OsxArm64),
            environment: Some("default".to_string()),
            workspace_config: WorkspaceConfig::default(),
        };
        let environment = workspace
            .environment_from_name_or_env_var(args.environment)
            .unwrap();
        let platform = args.platform.unwrap_or_else(|| environment.best_platform());

        let env_yaml = build_env_yaml(
            &platform,
            &environment,
            workspace.config().global_channel_config(),
        );
        insta::assert_snapshot!(
            "test_export_conda_env_yaml_pyproject_panic",
            env_yaml.unwrap().to_yaml_string()
        );
    }

    #[test]
    fn test_export_conda_env_yaml_with_defaults() {
        let toml = r#"
            [workspace]
            name = "test"
            channels = ["main"]
            platforms = ["osx-64"]

            [dependencies]
            python = "3.9"
           "#;
        let workspace = Workspace::from_str(Path::new("pixi.toml"), toml).unwrap();
        let args = Args {
            output_path: None,
            platform: Some(Platform::Osx64),
            environment: None,
            workspace_config: WorkspaceConfig::default(),
        };
        let environment = workspace
            .environment_from_name_or_env_var(args.environment)
            .unwrap();
        let platform = args.platform.unwrap_or_else(|| environment.best_platform());

        let env_yaml = build_env_yaml(
            &platform,
            &environment,
            workspace.config().global_channel_config(),
        );
        insta::assert_snapshot!(
            "test_export_conda_env_yaml_with_defaults",
            env_yaml.unwrap().to_yaml_string()
        );
    }

    #[test]
    fn test_channels_with_nodefaults() {
        let channels = vec![NamedChannelOrUrl::Name("main".to_string())];
        let channels = channels_with_nodefaults(channels);
        assert_eq!(channels, vec![NamedChannelOrUrl::Name("main".to_string())]);

        let channels = vec![NamedChannelOrUrl::Name("conda-forge".to_string())];
        let channels = channels_with_nodefaults(channels);
        assert_eq!(
            channels,
            vec![
                NamedChannelOrUrl::Name("conda-forge".to_string()),
                NamedChannelOrUrl::Name("nodefaults".to_string())
            ]
        );
    }
}
