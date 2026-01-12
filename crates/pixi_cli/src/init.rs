use std::{cmp::PartialEq, path::PathBuf, str::FromStr};

use clap::{Parser, ValueEnum};
use pixi_api::{WorkspaceContext, workspace::InitOptions};
use rattler_conda_types::NamedChannelOrUrl;

use crate::cli_interface::CliInterface;

/// Creates a new workspace
///
/// This command is used to create a new workspace.
/// It prepares a manifest and some helpers for the user to start working.
///
/// As pixi can both work with `pixi.toml` and `pyproject.toml` files, the user
/// can choose which one to use with `--format`.
///
/// You can import an existing conda environment file with the `--import` flag.
#[derive(Parser, Debug)]
pub struct Args {
    /// Where to place the workspace (defaults to current path)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Channel to use in the workspace.
    #[arg(
        short,
        long = "channel",
        value_name = "CHANNEL",
        conflicts_with = "ENVIRONMENT_FILE"
    )]
    pub channels: Option<Vec<NamedChannelOrUrl>>,

    /// Platforms that the workspace supports.
    #[arg(short, long = "platform", id = "PLATFORM")]
    pub platforms: Vec<String>,

    /// Environment.yml file to bootstrap the workspace.
    #[arg(short = 'i', long = "import", id = "ENVIRONMENT_FILE")]
    pub env_file: Option<PathBuf>,

    /// The manifest format to create.
    #[arg(long, conflicts_with_all = ["ENVIRONMENT_FILE", "pyproject_toml"], ignore_case = true)]
    pub format: Option<ManifestFormat>,

    /// Create a pyproject.toml manifest instead of a pixi.toml manifest
    // BREAK (0.27.0): Remove this option from the cli in favor of the `format` option.
    #[arg(long, conflicts_with_all = ["ENVIRONMENT_FILE", "format"], alias = "pyproject", hide = true)]
    pub pyproject_toml: bool,

    /// Source Control Management used for this workspace
    #[arg(short = 's', long = "scm", ignore_case = true)]
    pub scm: Option<GitAttributes>,

    /// Set a mapping between conda channels and pypi channels.
    #[arg(long = "conda-pypi-map", value_parser = parse_conda_pypi_mapping, value_delimiter = ',')]
    pub conda_pypi_map: Option<Vec<(NamedChannelOrUrl, String)>>,

    /// Name of the workspace to create. If provided, the workspace will be registered in the
    /// global workspace registry.
    #[arg(short, long)]
    pub name: Option<String>,
}

fn parse_conda_pypi_mapping(s: &str) -> Result<(NamedChannelOrUrl, String), String> {
    s.split_once('=')
        .map(|(k, v)| {
            NamedChannelOrUrl::from_str(k)
                .map_err(|err| err.to_string())
                .map(|value| (value, v.to_string()))
        })
        .transpose()?
        .ok_or("expected KEY=VALUE".into())
}

#[derive(Parser, Debug, Clone, PartialEq, ValueEnum)]
pub enum ManifestFormat {
    Pixi,
    Pyproject,
    Mojoproject,
}

#[derive(Parser, Debug, Clone, PartialEq, ValueEnum)]
pub enum GitAttributes {
    Github,
    Gitlab,
    Codeberg,
}

impl From<Args> for InitOptions {
    fn from(args: Args) -> Self {
        let format = args.format.map(|f| match f {
            ManifestFormat::Pixi => pixi_api::workspace::ManifestFormat::Pixi,
            ManifestFormat::Pyproject => pixi_api::workspace::ManifestFormat::Pyproject,
            ManifestFormat::Mojoproject => pixi_api::workspace::ManifestFormat::Mojoproject,
        });

        let scm = args.scm.map(|s| match s {
            GitAttributes::Github => pixi_api::workspace::GitAttributes::Github,
            GitAttributes::Gitlab => pixi_api::workspace::GitAttributes::Gitlab,
            GitAttributes::Codeberg => pixi_api::workspace::GitAttributes::Codeberg,
        });

        InitOptions {
            path: args.path,
            channels: args.channels,
            platforms: args.platforms,
            env_file: args.env_file,
            format,
            scm,
            conda_pypi_mapping: args.conda_pypi_map.map(|map| map.into_iter().collect()),
            name: args.name,
        }
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let uses_deprecated_pyproject_flag = args.pyproject_toml;
    let mut options: InitOptions = args.into();

    // Deprecation warning for the `pyproject` option
    if uses_deprecated_pyproject_flag {
        eprintln!(
            "{}The '{}' option is deprecated and will be removed in the future.\nUse '{}' instead.",
            console::style(console::Emoji("⚠️ ", "")).yellow(),
            console::style("--pyproject").bold().red(),
            console::style("--format pyproject").bold().green(),
        );
        options.format = Some(pixi_api::workspace::ManifestFormat::Pyproject);
    }

    WorkspaceContext::init(CliInterface {}, options).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multiple_format_values() {
        let test_cases = vec![
            ("pixi", ManifestFormat::Pixi),
            ("PiXi", ManifestFormat::Pixi),
            ("PIXI", ManifestFormat::Pixi),
            ("pyproject", ManifestFormat::Pyproject),
            ("PyPrOjEcT", ManifestFormat::Pyproject),
            ("PYPROJECT", ManifestFormat::Pyproject),
        ];

        for (input, expected) in test_cases {
            let args = Args::try_parse_from(["init", "--format", input]).unwrap();
            assert_eq!(args.format, Some(expected));
        }
    }

    #[test]
    fn test_multiple_scm_values() {
        let test_cases = vec![
            ("github", GitAttributes::Github),
            ("GiThUb", GitAttributes::Github),
            ("GITHUB", GitAttributes::Github),
            ("Github", GitAttributes::Github),
            ("gitlab", GitAttributes::Gitlab),
            ("GiTlAb", GitAttributes::Gitlab),
            ("GITLAB", GitAttributes::Gitlab),
            ("codeberg", GitAttributes::Codeberg),
            ("CoDeBeRg", GitAttributes::Codeberg),
            ("CODEBERG", GitAttributes::Codeberg),
        ];

        for (input, expected) in test_cases {
            let args = Args::try_parse_from(["init", "--scm", input]).unwrap();
            assert_eq!(args.scm, Some(expected));
        }
    }

    #[test]
    fn test_invalid_scm_values() {
        let invalid_values = vec!["invalid", "", "git", "bitbucket", "mercurial", "svn"];

        for value in invalid_values {
            let result = Args::try_parse_from(["init", "--scm", value]);
            assert!(
                result.is_err(),
                "Expected error for invalid SCM value '{value}', but got success"
            );
        }
    }
}
