use std::{io::Write, path::PathBuf};

use clap::Parser;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_core::WorkspaceLocator;
use pixi_manifest::HasWorkspaceManifest;
use pixi_spec::{GitReference, GitSpec, PathSourceSpec, PixiSpec, Subdirectory};
use rattler_conda_types::PackageName;
use url::Url;

use crate::{
    add::{ensure_pixi_build_preview_enabled, manifest_path_string, resolve_dependency_path},
    cli_config::{GitRev, WorkspaceConfig, warn_deprecated_subdir},
    has_specs::HasSpecs,
};

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

    /// The local path to use when adding a workspace path dependency.
    #[arg(long, conflicts_with = "git")]
    pub path: Option<PathBuf>,

    /// The git url to use when adding a workspace git dependency.
    #[clap(long, short, help_heading = pixi_consts::consts::CLAP_GIT_OPTIONS)]
    pub git: Option<Url>,

    #[clap(flatten)]
    /// The git revisions to use when adding a workspace git dependency.
    pub rev: Option<GitRev>,

    /// The subdirectory of the git repository to use.
    #[clap(long = "subdirectory", requires = "git", help_heading = pixi_consts::consts::CLAP_GIT_OPTIONS)]
    pub subdirectory: Option<String>,

    /// Deprecated alias for `--subdirectory`.
    #[clap(
        long = "subdir",
        hide = true,
        requires = "git",
        conflicts_with = "subdirectory"
    )]
    pub subdir: Option<String>,
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

impl AddArgs {
    fn has_source_dependency(&self) -> bool {
        self.path.is_some() || self.git.is_some()
    }

    fn subdirectory(&self) -> Option<String> {
        self.subdirectory.clone().or_else(|| self.subdir.clone())
    }

    fn warn_deprecated_subdir(&self) {
        warn_deprecated_subdir(self.subdir.as_deref());
    }
}

impl HasSpecs for AddArgs {
    fn packages(&self) -> Vec<&str> {
        self.specs.iter().map(AsRef::as_ref).collect()
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    match args.command {
        Command::Add(add_args) => {
            add_args.warn_deprecated_subdir();
            if add_args.has_source_dependency() && add_args.specs.len() != 1 {
                return Err(miette::miette!(
                    "source dependencies require exactly one package name"
                ));
            }

            if add_args.has_source_dependency() {
                workspace = ensure_pixi_build_preview_enabled(
                    workspace,
                    &args.config_source,
                    args.workspace_config.workspace_locator_start(),
                    pixi_config::ConfigCli::default(),
                )
                .await?;
                if let Some(backend_override) = args.workspace_config.backend_override.clone() {
                    workspace = workspace.with_backend_override(backend_override);
                }
            }

            let resolved_path = add_args
                .path
                .as_deref()
                .map(|path| resolve_dependency_path(path, &workspace, false))
                .transpose()?;
            let subdirectory = add_args
                .subdirectory()
                .map(Subdirectory::try_from)
                .transpose()
                .into_diagnostic()?
                .unwrap_or_default();
            let git_reference: GitReference = add_args.rev.clone().unwrap_or_default().into();

            let channel_config = workspace.channel_config();
            let specs = add_args.specs()?;
            let mut workspace = workspace.modify()?;
            {
                let manifest = workspace.manifest();
                for (name, spec) in specs {
                    let pixi_spec = if let Some(path) = &resolved_path {
                        PixiSpec::PathSource(Box::new(PathSourceSpec::new(manifest_path_string(
                            path,
                        ))))
                    } else if let Some(git) = &add_args.git {
                        PixiSpec::Git(Box::new(GitSpec::new(
                            git.clone(),
                            Some(git_reference.clone()),
                            subdirectory.clone(),
                        )))
                    } else {
                        let (_, nameless) = spec.into_nameless();
                        PixiSpec::from_nameless_matchspec(nameless, &channel_config)
                    };
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

    fn write_workspace_manifest(manifest_path: &Path, preview: bool) {
        let preview = if preview {
            "preview = [\"pixi-build\"]"
        } else {
            ""
        };
        fs_err::write(
            manifest_path,
            format!(
                r#"
            [workspace]
            name = "test"
            channels = []
            platforms = ["linux-64"]
            {preview}
            "#
            )
            .lines()
            .map(|line| line.trim_start())
            .collect::<Vec<_>>()
            .join("\n"),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn add_and_remove_workspace_dependencies() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join("pixi.toml");
        write_workspace_manifest(&manifest_path, false);

        execute(args(
            manifest_path.clone(),
            Command::Add(AddArgs {
                specs: vec!["numpy=1.*".to_string(), "boltons>=24".to_string()],
                path: None,
                git: None,
                rev: None,
                subdirectory: None,
                subdir: None,
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

    #[tokio::test]
    async fn add_workspace_dependency_with_path() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join("pixi.toml");
        write_workspace_manifest(&manifest_path, true);

        let package_dir = tempdir.path().join("local-package");
        fs_err::create_dir(&package_dir).unwrap();
        fs_err::write(
            package_dir.join("pixi.toml"),
            r#"
            [package]
            name = "local-package"
            version = "0.1.0"
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
                specs: vec!["local-package".to_string()],
                path: Some(package_dir),
                git: None,
                rev: None,
                subdirectory: None,
                subdir: None,
            }),
        ))
        .await
        .unwrap();

        let contents = fs_err::read_to_string(&manifest_path).unwrap();
        assert!(contents.contains("[workspace.dependencies]"));
        assert!(contents.contains(r#"local-package = { path = "local-package" }"#));
    }

    #[tokio::test]
    async fn add_workspace_dependency_with_git() {
        let tempdir = tempfile::tempdir().unwrap();
        let manifest_path = tempdir.path().join("pixi.toml");
        write_workspace_manifest(&manifest_path, true);

        execute(args(
            manifest_path.clone(),
            Command::Add(AddArgs {
                specs: vec!["local-package".to_string()],
                path: None,
                git: Some(Url::parse("https://github.com/example/local-package").unwrap()),
                rev: Some(GitRev::new().with_tag("v1.2.3".to_string())),
                subdirectory: Some("recipe".to_string()),
                subdir: None,
            }),
        ))
        .await
        .unwrap();

        let contents = fs_err::read_to_string(&manifest_path).unwrap();
        assert!(contents.contains("[workspace.dependencies]"));
        assert!(contents.contains(
            r#"local-package = { git = "https://github.com/example/local-package", tag = "v1.2.3", subdirectory = "recipe" }"#
        ));
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
