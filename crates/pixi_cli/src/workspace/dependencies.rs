use std::io::Write;

use clap::Parser;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_core::WorkspaceLocator;
use pixi_manifest::HasWorkspaceManifest;
use pixi_spec::PixiSpec;
use rattler_conda_types::PackageName;

use crate::{cli_config::WorkspaceConfig, has_specs::HasSpecs};

/// Commands to manage the `[workspace.dependencies]` table.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub struct AddArgs {
    /// The dependency as names or conda MatchSpecs.
    #[arg(required = true, value_name = "SPEC")]
    pub specs: Vec<String>,
}

#[derive(Parser, Debug)]
pub struct RemoveArgs {
    /// The dependency names to remove from `[workspace.dependencies]`.
    #[arg(required = true, value_name = "PACKAGE")]
    pub packages: Vec<PackageName>,
}

#[derive(Parser, Debug, Default)]
pub struct ListArgs {
    /// Output the dependency names in machine readable format (space delimited).
    /// This output is used for autocomplete.
    #[arg(long, hide(true))]
    pub machine_readable: bool,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Add dependencies to the `[workspace.dependencies]` table.
    #[clap(visible_alias = "a")]
    Add(AddArgs),
    /// List dependencies in the `[workspace.dependencies]` table.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
    /// Remove dependencies from the `[workspace.dependencies]` table.
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
}

impl HasSpecs for AddArgs {
    fn packages(&self) -> Vec<&str> {
        self.specs.iter().map(AsRef::as_ref).collect()
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    match args.command {
        Command::Add(add_args) => {
            let channel_config = workspace.channel_config();
            let specs = add_args.specs()?;
            let mut workspace = workspace.modify()?;
            {
                let manifest = workspace.manifest();
                for (name, spec) in specs {
                    let (_, nameless) = spec.into_nameless();
                    let pixi_spec = PixiSpec::from_nameless_matchspec(nameless, &channel_config);
                    manifest
                        .document
                        .add_workspace_dependency(&name, &pixi_spec)?;
                }
            }
            workspace.save().await.into_diagnostic()?;

            for spec in &add_args.specs {
                eprintln!(
                    "{}Added {} to {}",
                    console::style(console::Emoji("✔ ", "")).green(),
                    console::style(spec).bold(),
                    console::style("[workspace.dependencies]").bold(),
                );
            }
            Ok(())
        }
        Command::List(list_args) => {
            let dependencies = &(&workspace).workspace_manifest().workspace.dependencies;
            let output = if list_args.machine_readable {
                dependencies
                    .keys()
                    .map(|name| name.as_normalized())
                    .join(" ")
            } else {
                format_workspace_dependencies(dependencies)?
            };
            writeln!(std::io::stdout(), "{output}")
                .inspect_err(|e| {
                    if e.kind() == std::io::ErrorKind::BrokenPipe {
                        std::process::exit(0);
                    }
                })
                .into_diagnostic()?;
            Ok(())
        }
        Command::Remove(remove_args) => {
            let mut workspace = workspace.modify()?;
            {
                let manifest = workspace.manifest();
                for package in &remove_args.packages {
                    manifest.document.remove_workspace_dependency(package)?;
                }
            }
            workspace.save().await.into_diagnostic()?;

            for package in &remove_args.packages {
                eprintln!(
                    "{}Removed {} from {}",
                    console::style(console::Emoji("✔ ", "")).green(),
                    console::style(package.as_normalized()).bold(),
                    console::style("[workspace.dependencies]").bold(),
                );
            }
            Ok(())
        }
    }
}

fn format_workspace_dependencies(
    dependencies: &IndexMap<PackageName, pixi_spec::TomlSpec>,
) -> miette::Result<String> {
    if dependencies.is_empty() {
        return Ok("Workspace dependencies: none".to_string());
    }

    let lines = dependencies
        .iter()
        .map(|(name, spec)| {
            let rendered = render_toml_spec(name, spec)?;
            Ok(format!("- {} = {}", name.as_normalized(), rendered))
        })
        .collect::<miette::Result<Vec<_>>>()?;

    Ok(format!("Workspace dependencies:\n{}", lines.join("\n")))
}

fn render_toml_spec(name: &PackageName, spec: &pixi_spec::TomlSpec) -> miette::Result<String> {
    let mut without_version = spec.clone();
    let version = without_version.version.take();
    if let Some(version) = version
        && without_version.is_empty()
    {
        return Ok(format!("\"{version}\""));
    }

    let spec = spec.clone().into_spec().map_err(|e| {
        miette::miette!(
            "failed to render workspace dependency `{}`: {e}",
            name.as_normalized()
        )
    })?;
    Ok(spec.to_toml_value().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_core::Workspace;
    use std::{
        path::{Path, PathBuf},
        str::FromStr,
    };

    #[test]
    fn format_empty_workspace_dependencies() {
        assert_eq!(
            format_workspace_dependencies(&IndexMap::new()).unwrap(),
            "Workspace dependencies: none"
        );
    }

    fn args(manifest_path: PathBuf, command: Command) -> Args {
        Args {
            config_source: pixi_config::ConfigSourceCli {
                no_config: true,
                config_file: None,
            },
            workspace_config: WorkspaceConfig {
                manifest_path: Some(manifest_path),
                workspace: None,
                backend_override: None,
            },
            command,
        }
    }

    #[test]
    fn workspace_dependency_aliases_parse() {
        use clap::Parser;

        assert!(matches!(
            super::super::Args::try_parse_from(["workspace", "dependencies", "list"])
                .unwrap()
                .command,
            super::super::Command::Dependencies(Args {
                command: Command::List(_),
                ..
            })
        ));
        assert!(matches!(
            super::super::Args::try_parse_from(["workspace", "dependency", "list"])
                .unwrap()
                .command,
            super::super::Command::Dependencies(Args {
                command: Command::List(_),
                ..
            })
        ));
        assert!(matches!(
            super::super::Args::try_parse_from(["workspace", "dep", "list"])
                .unwrap()
                .command,
            super::super::Command::Dependencies(Args {
                command: Command::List(_),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn add_and_remove_workspace_dependencies() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join("pixi.toml");
        fs_err::write(
            &manifest_path,
            r#"
            [workspace]
            name = "test"
            channels = []
            platforms = ["linux-64"]
            "#
            .lines()
            .map(|line| line.trim_start())
            .collect::<Vec<_>>()
            .join("\n"),
        )
        .unwrap();

        execute(args(
            manifest_path.clone(),
            Command::Add(AddArgs {
                specs: vec!["numpy=1.*".to_string(), "boltons>=24".to_string()],
            }),
        ))
        .await
        .unwrap();

        let contents = fs_err::read_to_string(&manifest_path).unwrap();
        assert!(contents.contains("[workspace.dependencies]"));
        assert!(contents.contains(r#"numpy = "1.*""#));
        assert!(contents.contains(r#"boltons = ">=24""#));

        execute(args(
            manifest_path.clone(),
            Command::Remove(RemoveArgs {
                packages: vec![PackageName::from_str("numpy").unwrap()],
            }),
        ))
        .await
        .unwrap();

        let contents = fs_err::read_to_string(&manifest_path).unwrap();
        assert!(!contents.contains("numpy"));
        assert!(contents.contains("boltons"));
    }

    #[test]
    fn format_workspace_dependency_list() {
        let workspace = Workspace::from_str(
            Path::new("pixi.toml"),
            r#"
            [workspace]
            name = "test"
            channels = []
            platforms = ["linux-64"]

            [workspace.dependencies]
            numpy = "1.*"
            boltons = { version = ">=24", channel = "conda-forge" }
            "#,
        )
        .unwrap();

        insta::assert_snapshot!(
            format_workspace_dependencies(
                &(&workspace).workspace_manifest().workspace.dependencies
            )
            .unwrap(), @r#"
        Workspace dependencies:
        - boltons = { version = ">=24", channel = "conda-forge" }
        - numpy = "1.*"
        "#);
    }
}
