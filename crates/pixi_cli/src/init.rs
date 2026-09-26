use std::{
    cmp::PartialEq,
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
    str::FromStr,
};

use clap::{Parser, ValueEnum, ValueHint};
use miette::IntoDiagnostic;
use pixi_api::{Interface, WorkspaceContext, workspace::InitOptions};
use pixi_manifest::{
    CondaPypiMap, CondaPypiMapEntry,
    script::{
        ScriptManifest,
        conda::{CondaScriptManifest, supported_extensions, template_for_extension},
    },
};
use rattler_conda_types::NamedChannelOrUrl;

use crate::cli_interface::CliInterface;

/// Creates a new workspace or script
///
/// The positional path starts a workspace, while `--script` writes a metadata
/// block into a single file: a PEP 723 block for a Python file, a
/// `conda-script` block for every other known extension. `--format` overrides
/// that default.
///
/// As pixi can both work with `pixi.toml` and `pyproject.toml` files, the user
/// can choose which one to use with `--format`.
///
/// You can import an existing conda environment file with the `--import` flag.
#[derive(Parser, Debug)]
pub struct Args {
    /// Where to place the workspace (defaults to the current path)
    pub path: Option<PathBuf>,

    /// Create a metadata block in a script instead of a workspace
    #[arg(
        short = 's',
        long,
        value_name = "PATH",
        conflicts_with_all = ["ENVIRONMENT_FILE", "PLATFORM", "pyproject_toml", "scm", "conda_pypi_map"]
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

#[derive(Parser, Debug, Clone, Copy, PartialEq, ValueEnum)]
pub enum ManifestFormat {
    Pixi,
    Pyproject,
    Mojoproject,
    Pep723,
    CondaScript,
}

impl ManifestFormat {
    /// Whether the format lives in a metadata block inside a script.
    fn is_script(self) -> bool {
        matches!(self, Self::Pep723 | Self::CondaScript)
    }

    /// The spelling the user passes to `--format`.
    fn flag_value(self) -> &'static str {
        match self {
            Self::Pixi => "pixi",
            Self::Pyproject => "pyproject",
            Self::Mojoproject => "mojoproject",
            Self::Pep723 => "pep723",
            Self::CondaScript => "conda-script",
        }
    }
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
            ManifestFormat::Pep723 | ManifestFormat::CondaScript => {
                unreachable!("script formats route to script initialization")
            }
        });

        let scm = args.scm.map(|s| match s {
            GitAttributes::Github => pixi_api::workspace::GitAttributes::Github,
            GitAttributes::Gitlab => pixi_api::workspace::GitAttributes::Gitlab,
            GitAttributes::Codeberg => pixi_api::workspace::GitAttributes::Codeberg,
        });

        InitOptions {
            path: args
                .path
                .expect("a workspace always initializes with a directory"),
            channels: args.channels,
            platforms: args.platforms,
            env_file: args.env_file,
            format,
            scm,
            conda_pypi_mapping: args.conda_pypi_map,
        }
    }
}

/// The extension of a path, or an empty string.
fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_owned()
}

fn is_python_extension(extension: &str) -> bool {
    extension.eq_ignore_ascii_case("py") || extension.eq_ignore_ascii_case("pyw")
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Deprecation warning for the `pyproject` option
    let format = if args.pyproject_toml {
        eprintln!(
            "{}The '{}' option is deprecated and will be removed in the future.\nUse '{}' instead.",
            console::style(console::Emoji("⚠️ ", "")).yellow(),
            console::style("--pyproject").bold().red(),
            console::style("--format pyproject").bold().green(),
        );
        Some(ManifestFormat::Pyproject)
    } else {
        args.format
    };

    if let Some(script) = args.script.clone() {
        // A script lives at one path, so a directory next to it has nothing
        // left to name.
        if let Some(directory) = &args.path {
            return Err(miette::miette!(
                help = format!(
                    "put the directory in the script path, like `pixi init --script {}`",
                    directory.join(&script).display()
                ),
                "`--script` and the workspace directory {} cannot be combined",
                directory.display()
            ));
        }
        return initialize_script(script, format, args).await;
    }

    if let Some(format) = format.filter(|format| format.is_script()) {
        return Err(miette::miette!(
            help = format!(
                "pass the file to create, like `pixi init --script main.R --format {}`",
                format.flag_value()
            ),
            "`--format {}` needs `--script`",
            format.flag_value()
        ));
    }

    let options = InitOptions::from(Args {
        path: Some(args.path.unwrap_or_else(|| PathBuf::from("."))),
        format,
        pyproject_toml: false,
        ..args
    });
    WorkspaceContext::init(CliInterface {}, options).await?;
    Ok(())
}

/// Gives a script its metadata block: PEP 723 for Python, `conda-script`
/// for every other known extension, with `--format` overriding the default.
async fn initialize_script(
    path: PathBuf,
    format: Option<ManifestFormat>,
    args: Args,
) -> miette::Result<()> {
    let extension = extension_of(&path);
    let is_python = is_python_extension(&extension);
    let channels = args
        .channels
        .unwrap_or_default()
        .into_iter()
        .map(|channel| channel.to_string())
        .collect::<Vec<_>>();

    match format {
        Some(ManifestFormat::Pep723) if !is_python => Err(miette::miette!(
            help = "PEP 723 blocks only exist in Python files, ending in `.py` or `.pyw`",
            "`--format pep723` does not apply to {}",
            path.display()
        )),
        Some(ManifestFormat::Pep723) | None if is_python => {
            initialize_pep723_script(path, &channels).await
        }
        Some(ManifestFormat::CondaScript) | None => {
            initialize_conda_script(path, &extension, &channels).await
        }
        Some(format) => Err(miette::miette!(
            help = format!(
                "workspace formats go with a directory, like `pixi init --format {} my_workspace`",
                format.flag_value()
            ),
            "`--format {}` does not apply to a script",
            format.flag_value()
        )),
    }
}

async fn initialize_pep723_script(path: PathBuf, channels: &[String]) -> miette::Result<()> {
    let path = std::path::absolute(path).into_diagnostic()?;
    // A file with a conda-script block must not get a PEP 723 block on top;
    // the two kinds cannot coexist in one file.
    if path.is_file() && crate::conda_script::detect_with_fallback(&path, false)?.is_some() {
        return Err(miette::miette!(
            help = "a file can carry either a PEP 723 block or a conda-script block, not both",
            "{} is already a conda-script",
            path.display()
        ));
    }
    let script = ScriptManifest::initialize(&path, channels)?;

    CliInterface::default()
        .success(&format!(
            "Initialized script at {}",
            script.path().display()
        ))
        .await;
    Ok(())
}

async fn initialize_conda_script(
    path: PathBuf,
    extension: &str,
    channels: &[String],
) -> miette::Result<()> {
    let Some(template) = template_for_extension(extension) else {
        return Err(miette::miette!(
            help = format!(
                "pixi knows the comment syntax of these extensions: {}",
                supported_extensions().join(", ")
            ),
            "no conda-script template exists for {}",
            path.display()
        ));
    };

    let script = CondaScriptManifest::initialize(&path, template, channels)?;

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
    fn script_formats_parse() {
        let args = Args::try_parse_from(["init", "--format", "conda-script"]).unwrap();
        assert_eq!(args.format, Some(ManifestFormat::CondaScript));
        let args = Args::try_parse_from(["init", "--format", "pep723"]).unwrap();
        assert_eq!(args.format, Some(ManifestFormat::Pep723));
    }

    #[test]
    fn manifest_path_is_not_an_option() {
        assert!(Args::try_parse_from(["init", "--manifest-path", "example.py"]).is_err());
    }

    #[test]
    fn script_uses_the_reserved_short_form() {
        let long = Args::try_parse_from(["init", "--script", "example.py"]).unwrap();
        assert_eq!(long.script.as_deref(), Some(Path::new("example.py")));
        assert_eq!(long.path, None);

        let short = Args::try_parse_from(["init", "-s", "example.py"]).unwrap();
        assert_eq!(short.script.as_deref(), Some(Path::new("example.py")));
    }

    /// The script path carries its own directory, so a second path has
    /// nothing left to name.
    #[tokio::test]
    async fn script_and_a_workspace_directory_point_at_the_combined_path() {
        let args =
            Args::try_parse_from(["init", "--script", "main.mojo", "some_directory"]).unwrap();
        let error = execute(args).await.unwrap_err();

        assert!(error.to_string().contains("cannot be combined"));
        let combined = Path::new("some_directory").join("main.mojo");
        assert!(format!("{error:?}").contains(combined.to_str().unwrap()));
        assert!(
            !Path::new("some_directory").exists(),
            "the failed run must not create anything"
        );
    }

    #[test]
    fn script_rejects_workspace_only_initialization_options() {
        for incompatible in [
            vec!["--import", "environment.yml"],
            vec!["--platform", "linux-64"],
            vec!["--scm", "github"],
            vec!["--conda-pypi-map", "false"],
        ] {
            let mut arguments = vec!["init", "--script", "example.py"];
            arguments.extend(incompatible.clone());
            assert!(
                Args::try_parse_from(arguments).is_err(),
                "{incompatible:?} must not combine with a script"
            );
        }
    }

    #[tokio::test]
    async fn script_rejects_workspace_formats() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("example.py");

        let args = Args::try_parse_from([
            "init",
            "--script",
            path.to_str().unwrap(),
            "--format",
            "pixi",
        ])
        .unwrap();
        let error = execute(args).await.unwrap_err();

        assert!(error.to_string().contains("does not apply to a script"));
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn script_formats_need_the_script_flag() {
        for format in ["pep723", "conda-script"] {
            let args = Args::try_parse_from(["init", "--format", format]).unwrap();
            let error = execute(args).await.unwrap_err();
            assert!(error.to_string().contains("needs `--script`"));
        }
    }

    #[tokio::test]
    async fn refuses_an_existing_file_without_a_template() {
        let directory = tempfile::tempdir().unwrap();
        let readme = directory.path().join("README.md");
        fs_err::write(&readme, "# Project\n").unwrap();

        let args = Args::try_parse_from(["init", "--script", readme.to_str().unwrap()]).unwrap();
        let error = execute(args).await.unwrap_err();
        assert!(error.to_string().contains("no conda-script template"));
        assert_eq!(fs_err::read_to_string(&readme).unwrap(), "# Project\n");
    }

    #[tokio::test]
    async fn initializes_a_conda_script_from_the_extension_template() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.R");

        let args = Args::try_parse_from(["init", "--script", path.to_str().unwrap()]).unwrap();
        execute(args).await.unwrap();

        insta::assert_snapshot!(fs_err::read_to_string(path).unwrap(), @r###"
        # /// conda-script
        # channels = ["conda-forge"]
        # entrypoint = "Rscript ${SCRIPT}"
        #
        # [dependencies]
        # r-base = "*"
        # /// end-conda-script

        cat("Hello from pixi!\n")
        "###);
    }

    #[tokio::test]
    async fn format_overrides_the_python_default() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.py");

        let args = Args::try_parse_from([
            "init",
            "--script",
            path.to_str().unwrap(),
            "--format",
            "conda-script",
        ])
        .unwrap();
        execute(args).await.unwrap();

        let contents = fs_err::read_to_string(path).unwrap();
        assert!(contents.starts_with("# /// conda-script\n"));
    }

    #[tokio::test]
    async fn pep723_format_needs_a_python_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.R");

        let args = Args::try_parse_from([
            "init",
            "--script",
            path.to_str().unwrap(),
            "--format",
            "pep723",
        ])
        .unwrap();
        let error = execute(args).await.unwrap_err();
        assert!(error.to_string().contains("does not apply"));
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn conda_script_keeps_an_existing_body_and_shebang() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tool.sh");
        fs_err::write(&path, "#!/usr/bin/env bash\necho hi\n").unwrap();

        let args = Args::try_parse_from(["init", "--script", path.to_str().unwrap()]).unwrap();
        execute(args).await.unwrap();

        insta::assert_snapshot!(fs_err::read_to_string(path).unwrap(), @r###"
        #!/usr/bin/env bash
        #
        # /// conda-script
        # channels = ["conda-forge"]
        # entrypoint = "brush ${SCRIPT}"
        #
        # [dependencies]
        # brush = "*"
        # /// end-conda-script

        echo hi
        "###);
    }

    #[tokio::test]
    async fn refuses_to_reinitialize_a_conda_script() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("main.R");

        let args = Args::try_parse_from(["init", "--script", path.to_str().unwrap()]).unwrap();
        execute(args).await.unwrap();
        let contents = fs_err::read_to_string(&path).unwrap();

        let args = Args::try_parse_from(["init", "--script", path.to_str().unwrap()]).unwrap();
        let error = execute(args).await.unwrap_err();
        assert!(error.to_string().contains("already a conda-script"));
        assert_eq!(fs_err::read_to_string(&path).unwrap(), contents);
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
