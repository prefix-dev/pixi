use std::{cmp::PartialEq, collections::HashMap, path::PathBuf, str::FromStr};

use clap::{Parser, ValueEnum, ValueHint};
use miette::IntoDiagnostic;
use pixi_api::{Interface, WorkspaceContext, workspace::InitOptions};
use pixi_manifest::{CondaPypiMap, CondaPypiMapEntry, script::ScriptManifest};
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
    #[arg(default_value = ".", conflicts_with = "script")]
    pub path: PathBuf,

    /// Create a PEP 723 metadata block in a Python script.
    #[arg(
        short = 's',
        long,
        value_name = "PATH",
        conflicts_with_all = [
            "ENVIRONMENT_FILE",
            "PLATFORM",
            "format",
            "pyproject_toml",
            "scm",
            "conda_pypi_map"
        ]
    )]
    pub script: Option<PathBuf>,

    /// Channel to use in the workspace.
    #[arg(
        short,
        long = "channel",
        value_name = "CHANNEL",
        conflicts_with = "ENVIRONMENT_FILE"
    )]
    pub channels: Option<Vec<NamedChannelOrUrl>>,

    /// Platforms that the workspace supports.
    #[arg(
        short,
        long = "platform",
        id = "PLATFORM",
        value_name = "NEW_PLATFORM",
        value_hint = ValueHint::Other
    )]
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
    #[arg(long = "scm", ignore_case = true)]
    pub scm: Option<GitAttributes>,

    /// Set conda↔PyPI mapping configuration.
    ///
    /// Use `false` to disable mapping, or `CHANNEL=LOCATION[,CHANNEL=LOCATION]`
    /// for per-channel mapping locations.
    #[arg(long = "conda-pypi-map", value_parser = parse_conda_pypi_mapping)]
    pub conda_pypi_map: Option<CondaPypiMap>,
}

fn parse_conda_pypi_mapping(s: &str) -> Result<CondaPypiMap, String> {
    let s = s.trim();
    if s == "false" {
        return Ok(CondaPypiMap::Disabled);
    }
    if s == "true" {
        return Err(
            "`true` is not supported; use `false` to disable the mapping, or CHANNEL=LOCATION"
                .to_string(),
        );
    }

    let mut mappings = HashMap::new();
    for entry in s.split(',') {
        let (channel, location) = entry
            .split_once('=')
            .ok_or_else(|| "expected `false` or CHANNEL=LOCATION".to_string())?;
        let channel = NamedChannelOrUrl::from_str(channel.trim()).map_err(|err| err.to_string())?;
        let location = location.trim();
        let entry = if location == "false" {
            CondaPypiMapEntry::Disabled
        } else {
            CondaPypiMapEntry::from_location(location.to_string())
        };
        mappings.insert(channel, entry);
    }

    Ok(CondaPypiMap::Map(mappings))
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
            conda_pypi_mapping: args.conda_pypi_map,
        }
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    if let Some(path) = args.script.clone() {
        return initialize_script(path, args.channels).await;
    }

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

async fn initialize_script(
    path: PathBuf,
    channels: Option<Vec<NamedChannelOrUrl>>,
) -> miette::Result<()> {
    let path = std::path::absolute(path).into_diagnostic()?;
    let channels = channels
        .unwrap_or_default()
        .into_iter()
        .map(|channel| channel.to_string())
        .collect::<Vec<_>>();
    let script = ScriptManifest::initialize(&path, &channels)?;

    CliInterface::default()
        .success(&format!(
            "Initialized script at {}",
            script.path().display()
        ))
        .await;
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

    #[test]
    fn script_uses_the_reserved_short_form() {
        let long = Args::try_parse_from(["init", "--script", "example.py"]).unwrap();
        assert_eq!(
            long.script.as_deref(),
            Some(std::path::Path::new("example.py"))
        );

        let short = Args::try_parse_from(["init", "-s", "example.py"]).unwrap();
        assert_eq!(
            short.script.as_deref(),
            Some(std::path::Path::new("example.py"))
        );
    }

    #[test]
    fn script_rejects_workspace_only_initialization_options() {
        for incompatible in [
            vec!["--format", "pixi"],
            vec!["--import", "environment.yml"],
            vec!["--platform", "linux-64"],
            vec!["--scm", "github"],
            vec!["--conda-pypi-map", "false"],
        ] {
            let mut arguments = vec!["init", "--script", "example.py"];
            arguments.extend(incompatible);
            assert!(Args::try_parse_from(arguments).is_err());
        }

        assert!(Args::try_parse_from(["init", "workspace", "--script", "example.py"]).is_err());
    }

    #[tokio::test]
    async fn snapshots_the_default_script_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/example.py");

        let args = Args::try_parse_from(["init", "--script", path.to_str().unwrap()]).unwrap();
        execute(args).await.unwrap();

        insta::assert_snapshot!(fs_err::read_to_string(path).unwrap(), @r###"
        # /// script
        # requires-python = ">=3.11"
        # dependencies = []
        # ///
        "###);
    }

    #[tokio::test]
    async fn snapshots_script_metadata_with_an_explicit_channel() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("example.py");

        let args = Args::try_parse_from([
            "init",
            "--script",
            path.to_str().unwrap(),
            "--channel",
            "conda-forge",
        ])
        .unwrap();
        execute(args).await.unwrap();

        insta::assert_snapshot!(fs_err::read_to_string(path).unwrap(), @r###"
        # /// script
        # requires-python = ">=3.11"
        # dependencies = []
        #
        # [tool.pixi.workspace]
        # channels = ["conda-forge"]
        # ///
        "###);
    }

    #[test]
    fn test_conda_pypi_map_false_value() {
        let args = Args::try_parse_from(["init", "--conda-pypi-map", "false"]).unwrap();
        assert_eq!(args.conda_pypi_map, Some(CondaPypiMap::Disabled));
    }

    #[test]
    fn test_conda_pypi_map_location_values() {
        let args = Args::try_parse_from([
            "init",
            "--conda-pypi-map",
            "conda-forge=cf.json,https://example.com/channel=custom.json",
        ])
        .unwrap();

        let Some(CondaPypiMap::Map(map)) = args.conda_pypi_map else {
            panic!("expected a per-channel map");
        };
        assert_eq!(
            map.get(&NamedChannelOrUrl::from_str("conda-forge").unwrap()),
            Some(&CondaPypiMapEntry::from_location("cf.json".to_string()))
        );
        assert_eq!(
            map.get(&NamedChannelOrUrl::from_str("https://example.com/channel").unwrap()),
            Some(&CondaPypiMapEntry::from_location("custom.json".to_string()))
        );
    }
}
