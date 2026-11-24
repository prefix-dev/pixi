use itertools::Itertools;
use miette::{Diagnostic, NamedSource, SourceSpan};
use pixi_config::Config;
use rattler_conda_types::{
    MatchSpec, NamedChannelOrUrl, ParseMatchSpecError, ParseStrictness::Lenient,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::{io::BufRead, path::Path, str::FromStr};
use thiserror::Error;

/// An error that occurs when reading or converting a conda environment file.
#[derive(Debug, Error, Diagnostic)]
pub enum CondaEnvFileError {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Yaml(#[from] Box<YamlParseError>),

    #[error("Can't parse '{spec}' as conda dependency")]
    CondaDependency {
        spec: String,
        #[source]
        source: ParseMatchSpecError,
    },

    #[error("can't parse '{channel}' as channel")]
    Channel {
        channel: String,
        #[source]
        source: url::ParseError,
    },

    #[error("Can't parse '{requirement}' as pypi dependency")]
    PypiDependency {
        requirement: String,
        #[source]
        source: Box<pep508_rs::Pep508Error>,
    },
}

#[derive(Debug, Error)]
#[error("Failed to parse '{path}' as a conda environment file")]
pub struct YamlParseError {
    #[source]
    source: serde_yaml::Error,
    src: NamedSource<String>,
    span: Option<SourceSpan>,
    path: PathBuf,
}

impl Diagnostic for YamlParseError {
    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        Some(&self.src)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = miette::LabeledSpan> + '_>> {
        self.span.as_ref().map(|span| {
            Box::new(std::iter::once(miette::LabeledSpan::new(
                Some("error occurred here".to_string()),
                span.offset(),
                span.len(),
            ))) as Box<dyn Iterator<Item = miette::LabeledSpan>>
        })
    }
}

impl YamlParseError {
    fn new(src: NamedSource<String>, source: serde_yaml::Error, path: PathBuf) -> Self {
        let span = source.location().map(|loc| {
            let start = loc.index();
            let end = start + 1; // Could expand this to a larger span if needed
            (start..end).into()
        });
        Self {
            src,
            source,
            span,
            path,
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct CondaEnvFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    channels: Vec<NamedChannelOrUrl>,
    dependencies: Vec<CondaEnvDep>,
    #[serde(default)]
    variables: HashMap<String, String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum CondaEnvDep {
    Conda(String),
    Pip {
        #[serde(default)]
        pip: Option<Vec<String>>,
    },
}

/// The conda dependencies, PyPI dependencies and channels of an environment file.
pub type ParsedDependencies = (
    Vec<MatchSpec>,
    Vec<pep508_rs::Requirement>,
    Vec<NamedChannelOrUrl>,
);

impl CondaEnvFile {
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    fn channels(&self) -> &Vec<NamedChannelOrUrl> {
        &self.channels
    }

    fn dependencies(&self) -> &Vec<CondaEnvDep> {
        &self.dependencies
    }

    pub fn variables(&self) -> HashMap<String, String> {
        self.variables.clone()
    }

    pub fn from_path(path: &Path) -> Result<Self, CondaEnvFileError> {
        let file = fs_err::File::open(path)?;
        let reader = std::io::BufReader::new(file);

        let lines = reader.lines().collect::<Result<Vec<String>, _>>()?;
        let mut s = String::new();
        for line in lines {
            if line.contains("- sel(") {
                tracing::warn!("Skipping micromamba sel(...) in line: \"{}\"", line.trim());
                tracing::warn!("Please add the dependencies manually");
                continue;
            }
            s.push_str(&line);
            s.push('\n');
        }
        let env_file: CondaEnvFile = match serde_yaml::from_str(&s) {
            Ok(env_file) => env_file,
            Err(e) => {
                let src = NamedSource::new(path.display().to_string(), s.to_string());
                return Err(Box::new(YamlParseError::new(src, e, path.to_path_buf())).into());
            }
        };
        Ok(env_file)
    }

    pub fn to_manifest(
        self: CondaEnvFile,
        config: &Config,
    ) -> Result<ParsedDependencies, CondaEnvFileError> {
        // TODO: should we be applying `config.channel_config` for parsed channels too?
        let mut channels = parse_channels(self.channels().clone());
        let (conda_deps, pip_deps, extra_channels) =
            parse_dependencies(self.dependencies().clone())?;

        channels.extend(extra_channels);
        let mut channels: Vec<_> = channels.into_iter().unique().collect();
        if channels.is_empty() {
            channels = config.default_channels();
        }

        Ok((conda_deps, pip_deps, channels))
    }
}

fn parse_dependencies(deps: Vec<CondaEnvDep>) -> Result<ParsedDependencies, CondaEnvFileError> {
    let mut conda_deps = Vec::new();
    let mut pip_deps = Vec::new();
    let mut picked_up_channels = Vec::new();
    for dep in deps {
        match dep {
            CondaEnvDep::Conda(d) => {
                let match_spec = MatchSpec::from_str(&d, Lenient).map_err(|source| {
                    CondaEnvFileError::CondaDependency {
                        spec: d.clone(),
                        source,
                    }
                })?;
                if let Some(channel) = &match_spec.channel {
                    picked_up_channels.push(
                        // named channels are given a url with default channel config in `MatchSpec::from_str`
                        NamedChannelOrUrl::from_str(channel.base_url.as_str()).map_err(
                            |source| CondaEnvFileError::Channel {
                                channel: channel.base_url.to_string(),
                                source,
                            },
                        )?,
                    );
                }
                conda_deps.push(match_spec);
            }
            CondaEnvDep::Pip { pip } => {
                let pip = pip.unwrap_or_default();
                pip_deps.extend(
                    pip.iter()
                        .map(|dep| {
                            pep508_rs::Requirement::from_str(dep).map_err(|source| {
                                CondaEnvFileError::PypiDependency {
                                    requirement: dep.clone(),
                                    source: Box::new(source),
                                }
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                )
            }
        }
    }

    Ok((conda_deps, pip_deps, picked_up_channels))
}

fn parse_channels(channels: Vec<NamedChannelOrUrl>) -> Vec<NamedChannelOrUrl> {
    let mut new_channels = vec![];
    for channel in channels {
        if channel.as_str() == "defaults" {
            // https://docs.anaconda.com/free/working-with-conda/reference/default-repositories/#active-default-channels
            new_channels.push(NamedChannelOrUrl::Name("main".to_string()));
            new_channels.push(NamedChannelOrUrl::Name("r".to_string()));
            new_channels.push(NamedChannelOrUrl::Name("msys2".to_string()));
        } else {
            new_channels.push(channel);
        }
    }
    new_channels
}

#[cfg(test)]
mod tests {
    use std::{io::Write, path::Path, str::FromStr};

    use rattler_conda_types::{Channel, ChannelConfig, MatchSpec, ParseStrictness::Strict};

    use super::*;

    #[test]
    fn test_parse_conda_env_file() {
        let example_conda_env_file = r#"
        name: pixi_example_project
        channels:
          - conda-forge
          - https://custom-server.com/channel
        dependencies:
          - python
          - pytorch::torchvision
          - conda-forge::pytest
          - wheel=0.31.1
          - sel(linux): blabla
          - foo >=1.2.3.*  # only valid when parsing in lenient mode
          - pip:
            - requests
            - deepobs @ git+https://git@github.com/fsschneider/DeepOBS.git@develop
            - torch==1.8.1
        "#;

        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(example_conda_env_file.as_bytes()).unwrap();
        let (_file, path) = f.into_parts();

        let conda_env_file_data = CondaEnvFile::from_path(&path).unwrap();
        let channel_config = ChannelConfig::default_with_root_dir(
            std::env::current_dir().expect("Could not get current directory"),
        );

        assert_eq!(conda_env_file_data.name(), Some("pixi_example_project"));
        assert_eq!(
            conda_env_file_data.channels(),
            &vec![
                NamedChannelOrUrl::from_str("conda-forge").unwrap(),
                NamedChannelOrUrl::from_str("https://custom-server.com/channel").unwrap(),
            ]
        );

        let config = Config::default();
        let (conda_deps, pip_deps, channels) = conda_env_file_data.to_manifest(&config).unwrap();

        assert_eq!(
            channels,
            vec![
                NamedChannelOrUrl::from_str("conda-forge").unwrap(),
                NamedChannelOrUrl::from_str("https://custom-server.com/channel").unwrap(),
                NamedChannelOrUrl::from_str(
                    Channel::from_str("pytorch", &channel_config)
                        .unwrap()
                        .base_url
                        .as_str()
                )
                .unwrap(),
                NamedChannelOrUrl::from_str(
                    Channel::from_str("conda-forge", &channel_config)
                        .unwrap()
                        .base_url
                        .as_str()
                )
                .unwrap(),
            ]
        );

        println!("{conda_deps:?}");
        assert_eq!(
            conda_deps,
            vec![
                MatchSpec::from_str("python", Strict).unwrap(),
                MatchSpec::from_str("pytorch::torchvision", Strict).unwrap(),
                MatchSpec::from_str("conda-forge::pytest", Strict).unwrap(),
                MatchSpec::from_str("wheel=0.31.1", Strict).unwrap(),
                MatchSpec::from_str("foo >=1.2.3", Strict).unwrap(),
            ]
        );

        assert_eq!(
            pip_deps,
            vec![
                pep508_rs::Requirement::from_str("requests").unwrap(),
                pep508_rs::Requirement::from_str(
                    "deepobs @ git+https://git@github.com/fsschneider/DeepOBS.git@develop"
                )
                .unwrap(),
                pep508_rs::Requirement::from_str("torch==1.8.1").unwrap(),
            ]
        );
    }

    #[test]
    fn test_import_from_env_yamls() {
        let test_files_path = Path::new(&env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("environment_yamls");

        let entries = match fs_err::read_dir(test_files_path) {
            Ok(entries) => entries,
            Err(e) => panic!("Failed to read directory: {e}"),
        };

        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.expect("Failed to read directory entry");
            if entry.path().is_file() {
                paths.push(entry.path());
            }
        }

        for path in paths {
            let env_info = CondaEnvFile::from_path(&path).unwrap();
            // Try `cargo insta test` to run all at once
            let snapshot_name = format!(
                "test_import_from_env_yaml.{}",
                path.file_name().unwrap().to_string_lossy()
            );

            insta::assert_debug_snapshot!(
                snapshot_name,
                (
                    parse_dependencies(env_info.dependencies().clone()).unwrap(),
                    parse_channels(env_info.channels().clone()),
                    env_info.name()
                )
            );
        }
    }

    #[test]
    fn test_parse_conda_env_file_with_explicit_pip_dep() {
        let example_conda_env_file = r#"
        name: pixi_example_project
        channels:
          - conda-forge
        dependencies:
          - pip==24.0
          - pip:
            - requests
        "#;

        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path();
        let mut file = fs_err::File::create(path).unwrap();
        file.write_all(example_conda_env_file.as_bytes()).unwrap();

        let conda_env_file_data = CondaEnvFile::from_path(path).unwrap();
        let vars = conda_env_file_data.variables();
        let (conda_deps, pip_deps, _) =
            parse_dependencies(conda_env_file_data.dependencies().clone()).unwrap();

        assert_eq!(
            conda_deps,
            vec![MatchSpec::from_str("pip==24.0", Strict).unwrap(),]
        );

        assert_eq!(
            pip_deps,
            vec![pep508_rs::Requirement::from_str("requests").unwrap()]
        );

        let empty_map = HashMap::<String, String>::new();

        assert_eq!(vars, empty_map);
    }

    #[test]
    fn test_parse_conda_env_file_with_variables() {
        let example_conda_env_file = r#"
        name: pixi_example_project
        channels:
          - conda-forge
        dependencies:
          - pip==24.0
        variables:
          MY_VAR: my_value
          MY_OTHER_VAR: 123
          MY_EMPTY_VAR:
        "#;

        let f = tempfile::NamedTempFile::new().unwrap();
        let path = f.path();
        let mut file = fs_err::File::create(path).unwrap();
        file.write_all(example_conda_env_file.as_bytes()).unwrap();

        let conda_env_file_data = CondaEnvFile::from_path(path).unwrap();
        let vars = conda_env_file_data.variables();

        assert_eq!(
            vars,
            HashMap::from([
                ("MY_VAR".to_string(), "my_value".to_string()),
                ("MY_OTHER_VAR".to_string(), "123".to_string()),
                ("MY_EMPTY_VAR".to_string(), "".to_string())
            ])
        );
    }

    #[test]
    fn test_invalid_yaml_keeps_source_span() {
        let f = tempfile::NamedTempFile::new().unwrap();
        fs_err::write(f.path(), "name: test\nchannels: 12\n").unwrap();

        let err = CondaEnvFile::from_path(f.path()).unwrap_err();
        assert!(matches!(err, CondaEnvFileError::Yaml(_)));
        assert!(err.source_code().is_some());
        assert_eq!(err.labels().unwrap().count(), 1);
    }
}
