use std::{io, path::Path};

use clap::Parser;
use fs_err::tokio as tokio_fs;
use miette::Diagnostic;
use url::Url;

use pixi_config::pixi_home;
use pixi_consts::consts;
use pixi_global::project::FromMatchSpecError;
use pixi_spec::{PixiSpec, Subdirectory, SubdirectoryError};
use rattler_conda_types::{
    ChannelConfig, MatchSpec, NamedChannelOrUrl, PackageName, ParseMatchSpecError,
    ParseMatchSpecOptions, RepodataRevision,
};
use typed_path::Utf8NativePathBuf;

use crate::has_specs::HasSpecs;

#[derive(Parser, Debug, Default, Clone)]
pub struct GlobalSpecs {
    /// The dependency as names, conda MatchSpecs
    #[arg(num_args = 1.., required_unless_present_any = ["git", "path"], value_name = "PACKAGE")]
    pub specs: Vec<String>,

    /// The git url, e.g. `https://github.com/user/repo.git`
    #[clap(long, help_heading = consts::CLAP_GIT_OPTIONS)]
    pub git: Option<Url>,

    /// The git revisions
    #[clap(flatten)]
    pub rev: Option<crate::cli_config::GitRev>,

    /// The subdirectory within the git repository
    #[clap(long, requires = "git", help_heading = consts::CLAP_GIT_OPTIONS)]
    pub subdir: Option<String>,

    /// The path to the local package
    #[clap(long, conflicts_with = "git")]
    pub path: Option<Utf8NativePathBuf>,

    /// The build backend to build the source with, when the source does not
    /// provide its own package manifest (or to override the one it has).
    /// Accepts a name with an optional version constraint, e.g.
    /// `pixi-build-rust` or `"pixi-build-rust>=0.3,<0.4"`.
    #[clap(long, value_name = "BUILD_BACKEND")]
    pub build_backend: Option<String>,

    /// Additional fields of the inline package definition, as
    /// `DOTTED_KEY=TOML_VALUE` pairs that are recorded under the `package`
    /// key of the dependency, e.g. `host-dependencies.hatchling="*"` or
    /// `build.config.extra-args=["--all-features"]`.
    #[clap(long = "package", value_name = "KEY=VALUE")]
    pub package: Vec<String>,
}

impl HasSpecs for GlobalSpecs {
    fn packages(&self) -> Vec<&str> {
        self.specs.iter().map(AsRef::as_ref).collect()
    }
}

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum GlobalSpecsConversionError {
    #[error(transparent)]
    ParseMatchSpecError(#[from] ParseMatchSpecError),
    #[error(transparent)]
    FromMatchSpec(#[from] Box<FromMatchSpecError>),
    #[error("package name is required when specifying version constraints without --git or --path")]
    #[diagnostic(
        help = "Use a full package specification like `python==3.12` instead of just `==3.12`"
    )]
    NameRequired,
    #[error("`--build-backend` and `--package` require a source location")]
    #[diagnostic(help = "Add `--git <URL>` or `--path <PATH>` to specify where the source lives")]
    InlineRequiresSource,
    #[error("invalid `--build-backend` value '{input}': {reason}")]
    #[diagnostic(
        help = "Pass a package name with an optional version constraint, e.g. `pixi-build-rust` or `\"pixi-build-rust>=0.3,<0.4\"`"
    )]
    InvalidBuildBackend { input: String, reason: String },
    #[error("invalid `--package` value '{input}': {reason}")]
    #[diagnostic(
        help = "Pass a `DOTTED_KEY=TOML_VALUE` pair, e.g. `--package 'host-dependencies.hatchling=\"*\"'`"
    )]
    InvalidPackageFragment { input: String, reason: String },
    #[error("the key '{key}' of the inline package definition is set more than once")]
    #[diagnostic(help = "Each `--package` key (and `--build-backend`) may only be set once")]
    PackageKeyCollision { key: String },
    #[error(transparent)]
    #[diagnostic(transparent)]
    InlinePackageValue(#[from] pixi_global::project::InlinePackageValueError),
    #[error("couldn't construct a relative path from {} to {}", .0, .1)]
    RelativePath(String, String),
    #[error("could not absolutize path: {0}")]
    AbsolutizePath(String),
    #[error("`.conda` path must include a file name: {0}")]
    CondaPathMissingFileName(String),
    #[error("failed to determine pixi home directory")]
    PixiHomeUnavailable,
    #[error("failed to create conda files directory at {path}")]
    CreateCondaFilesDir {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("failed to copy conda package from {src} to {dst}")]
    CopyCondaFile {
        src: String,
        dst: String,
        #[source]
        source: io::Error,
    },
    #[error("failed to infer package name")]
    #[diagnostic(transparent)]
    PackageNameInference(#[from] pixi_global::project::InferPackageNameError),
    #[error("Input {0} looks like a path: please pass `--path`.")]
    MissingPathArg(String),
    #[error(transparent)]
    InvalidSubdirectory(#[from] SubdirectoryError),
}

/// Merges `source` into `target` one level at a time: when both sides hold a
/// table under the same key, those tables are merged as well, while a key that
/// is already set to anything else is a collision and errors out. That way
/// several `--package` fragments can build up a single definition without
/// silently overwriting each other's keys.
/// `path` is the dotted key of the table being merged right now, for example
/// `build.backend`, so a collision can be reported as the full key.
fn merge_inline_tables(
    target: &mut toml_edit::InlineTable,
    source: toml_edit::InlineTable,
    path: &str,
) -> Result<(), GlobalSpecsConversionError> {
    for (key, value) in source.into_iter() {
        let key_path = if path.is_empty() {
            key.to_string()
        } else {
            format!("{path}.{key}")
        };
        match target.get_mut(&key) {
            None => {
                target.insert(&key, value);
            }
            Some(toml_edit::Value::InlineTable(existing)) => {
                if let toml_edit::Value::InlineTable(source) = value {
                    merge_inline_tables(existing, source, &key_path)?;
                } else {
                    return Err(GlobalSpecsConversionError::PackageKeyCollision { key: key_path });
                }
            }
            Some(_) => {
                return Err(GlobalSpecsConversionError::PackageKeyCollision { key: key_path });
            }
        }
    }
    Ok(())
}

/// Parses a single `--package DOTTED_KEY=TOML_VALUE` fragment into an inline
/// table. The right-hand side must be a valid TOML value, so strings need
/// their quotes: `--package 'build.config.profile="release"'`.
fn parse_package_fragment(
    input: &str,
) -> Result<toml_edit::InlineTable, GlobalSpecsConversionError> {
    let document = input.parse::<toml_edit::DocumentMut>().map_err(|error| {
        GlobalSpecsConversionError::InvalidPackageFragment {
            input: input.to_string(),
            reason: error.message().to_string(),
        }
    })?;
    Ok(document.as_table().clone().into_inline_table())
}

/// Renders a chain of single-entry tables as dotted keys, so a definition
/// shows up as `package.build.backend.name = "pixi-build-rust"` instead of
/// `package = { build = { backend = { name = "pixi-build-rust" } } }`.
/// Tables with several entries keep their braces.
fn collapse_to_dotted_keys(table: &mut toml_edit::InlineTable) {
    if table.len() != 1 {
        return;
    }
    if let Some((_, value)) = table.iter_mut().next()
        && let Some(nested) = value.as_inline_table_mut()
    {
        collapse_to_dotted_keys(nested);
    }
    table.set_dotted(true);
}

impl GlobalSpecs {
    /// Builds the inline package definition from `--build-backend` and
    /// `--package`, or `None` when neither is given.
    fn inline_package_value(
        &self,
    ) -> Result<Option<pixi_global::project::InlinePackageValue>, GlobalSpecsConversionError> {
        if self.build_backend.is_none() && self.package.is_empty() {
            return Ok(None);
        }

        let mut package = toml_edit::InlineTable::new();

        // `--build-backend NAME` is sugar for
        // `--package 'build.backend.name="NAME"'` (plus the version
        // constraint when one is given).
        if let Some(input) = &self.build_backend {
            let match_spec =
                MatchSpec::from_str(input, ParseMatchSpecOptions::lenient()).map_err(|e| {
                    GlobalSpecsConversionError::InvalidBuildBackend {
                        input: input.clone(),
                        reason: e.to_string(),
                    }
                })?;
            let name = match_spec.name.as_exact().cloned().ok_or_else(|| {
                GlobalSpecsConversionError::InvalidBuildBackend {
                    input: input.clone(),
                    reason: "a package name is required".to_string(),
                }
            })?;
            let name_and_version_only = MatchSpec {
                name: match_spec.name.clone(),
                version: match_spec.version.clone(),
                ..MatchSpec::default()
            };
            if match_spec != name_and_version_only {
                return Err(GlobalSpecsConversionError::InvalidBuildBackend {
                    input: input.clone(),
                    reason: "only a name and a version constraint are supported".to_string(),
                });
            }

            let mut backend = toml_edit::InlineTable::new();
            backend.insert("name", name.as_source().into());
            if let Some(version) = &match_spec.version {
                backend.insert("version", version.to_string().into());
            }
            let mut build = toml_edit::InlineTable::new();
            build.insert("backend", toml_edit::Value::InlineTable(backend));
            package.insert("build", toml_edit::Value::InlineTable(build));
        }

        for fragment in &self.package {
            let table = parse_package_fragment(fragment)?;
            merge_inline_tables(&mut package, table, "")?;
        }

        collapse_to_dotted_keys(&mut package);

        Ok(Some(pixi_global::project::InlinePackageValue::new(package)))
    }

    /// Convert GlobalSpecs to a vector of GlobalSpec instances. `channels`
    /// are the channels of the environment the specs are destined for; name
    /// inference solves the build backend against them.
    pub async fn to_global_specs(
        &self,
        channel_config: &ChannelConfig,
        manifest_root: &Path,
        project: &pixi_global::Project,
        channels: &[NamedChannelOrUrl],
    ) -> Result<Vec<pixi_global::project::GlobalSpec>, GlobalSpecsConversionError> {
        let git_or_path_spec = if let Some(git_url) = &self.git {
            let git_spec = pixi_spec::GitSpec::new(
                git_url.clone(),
                self.rev.clone().map(Into::into),
                self.subdir
                    .clone()
                    .map(Subdirectory::try_from)
                    .transpose()?
                    .unwrap_or_default(),
            );
            Some(PixiSpec::from(git_spec))
        } else if let Some(path) = &self.path {
            let absolute_path = dunce::canonicalize(path.as_str())
                .map_err(|_| GlobalSpecsConversionError::AbsolutizePath(path.to_string()))?;

            let is_conda = absolute_path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("conda"));

            let absolute_path = if is_conda {
                let pixi_home_dir =
                    pixi_home().ok_or(GlobalSpecsConversionError::PixiHomeUnavailable)?;
                let conda_files_dir = pixi_home_dir.join("conda-files");
                tokio_fs::create_dir_all(&conda_files_dir)
                    .await
                    .map_err(|source| GlobalSpecsConversionError::CreateCondaFilesDir {
                        path: conda_files_dir.to_string_lossy().to_string(),
                        source,
                    })?;

                let file_name = absolute_path.file_name().ok_or_else(|| {
                    GlobalSpecsConversionError::CondaPathMissingFileName(
                        absolute_path.to_string_lossy().to_string(),
                    )
                })?;

                let destination = conda_files_dir.join(file_name);
                if absolute_path != destination {
                    tokio_fs::copy(&absolute_path, &destination)
                        .await
                        .map_err(|source| GlobalSpecsConversionError::CopyCondaFile {
                            src: absolute_path.to_string_lossy().to_string(),
                            dst: destination.to_string_lossy().to_string(),
                            source,
                        })?;
                }

                dunce::canonicalize(&destination).map_err(|_| {
                    GlobalSpecsConversionError::AbsolutizePath(
                        destination.to_string_lossy().to_string(),
                    )
                })?
            } else {
                absolute_path
            };

            let relative_path =
                pathdiff::diff_paths(&absolute_path, manifest_root).ok_or_else(|| {
                    GlobalSpecsConversionError::RelativePath(
                        absolute_path.to_string_lossy().to_string(),
                        manifest_root.to_string_lossy().to_string(),
                    )
                })?;
            Some(PixiSpec::from(pixi_spec::PathSpec::new(
                Utf8NativePathBuf::from(relative_path.to_string_lossy().to_string())
                    .to_typed_path_buf(),
            )))
        } else {
            fn pathlike(s: &str) -> bool {
                s.contains(".conda") || s.contains('/') || s.contains('\\')
            }
            if let Some(pathlike_input) = self.specs.iter().find(|s| pathlike(s)) {
                return Err(GlobalSpecsConversionError::MissingPathArg(
                    pathlike_input.clone(),
                ));
            }
            None
        };
        // The inline package definition assembled from `--build-backend` and
        // `--package`, if any. It only makes sense next to a source location.
        let inline = self.inline_package_value()?;

        if let Some(pixi_spec) = git_or_path_spec {
            if self.specs.is_empty() {
                // Infer the package name from the path/git spec. With an
                // inline definition present, backend discovery uses it
                // instead of reading a manifest from the checkout; the
                // placeholder name only serves the inference call, the
                // definition is re-anchored to the inferred name below.
                let inline_manifest = inline
                    .as_ref()
                    .map(|value| {
                        value.to_inline_manifest(
                            &PackageName::new_unchecked("uninferred-package"),
                            manifest_root,
                        )
                    })
                    .transpose()?;
                let inferred_name = project
                    .infer_package_name_from_spec(&pixi_spec, inline_manifest.as_ref(), channels)
                    .await?;
                let mut spec = pixi_global::project::GlobalSpec::new(inferred_name, pixi_spec);
                if let Some(inline) = inline {
                    spec = spec.with_inline(inline);
                }
                return Ok(vec![spec]);
            }

            self.specs
                .iter()
                .map(|spec_str| {
                    let name = MatchSpec::from_str(
                        spec_str,
                        ParseMatchSpecOptions::lenient()
                            .with_repodata_revision(RepodataRevision::V3),
                    )?
                    .name
                    .as_exact()
                    .cloned()
                    .ok_or(GlobalSpecsConversionError::NameRequired)?;
                    // Validate the inline definition early, before anything
                    // is recorded in the manifest.
                    if let Some(inline) = &inline {
                        inline.to_inline_manifest(&name, manifest_root)?;
                    }
                    let mut spec = pixi_global::project::GlobalSpec::new(name, pixi_spec.clone());
                    if let Some(inline) = &inline {
                        spec = spec.with_inline(inline.clone());
                    }
                    Ok(spec)
                })
                .collect()
        } else if inline.is_some() {
            Err(GlobalSpecsConversionError::InlineRequiresSource)
        } else {
            self.specs
                .iter()
                .map(|spec_str| {
                    pixi_global::project::GlobalSpec::try_from_str(spec_str, channel_config)
                        .map_err(Box::new)
                        .map_err(GlobalSpecsConversionError::FromMatchSpec)
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use fs_err as fs;
    use rattler_conda_types::ChannelConfig;
    use temp_env;
    use tempfile::tempdir;

    use super::*;

    /// Create a project in an isolated `PIXI_HOME` so tests never read the
    /// global manifest of the machine they run on.
    async fn isolated_project(temp_dir: &Path) -> pixi_global::Project {
        let pixi_home_dir = temp_dir.join("pixi-home");
        temp_env::async_with_vars(
            [("PIXI_HOME", Some(pixi_home_dir.to_str().unwrap()))],
            pixi_global::Project::discover_or_create(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn test_to_global_specs_named() {
        let specs = GlobalSpecs {
            specs: vec!["numpy==1.21.0".to_string(), "scipy>=1.7".to_string()],
            ..Default::default()
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let manifest_root = PathBuf::from(".");

        // Create a dummy project - this test doesn't use path/git specs so project
        // won't be called
        let temp_dir = tempdir().unwrap();
        let project = isolated_project(temp_dir.path()).await;

        let global_specs = specs
            .to_global_specs(&channel_config, &manifest_root, &project, &[])
            .await
            .unwrap();

        assert_eq!(global_specs.len(), 2);
    }

    #[tokio::test]
    async fn test_to_global_specs_with_git() {
        let specs = GlobalSpecs {
            specs: vec!["mypackage".to_string()],
            git: Some("https://github.com/user/repo.git".parse().unwrap()),
            ..Default::default()
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let manifest_root = PathBuf::from(".");

        // Create a dummy project - this test specifies a package name so inference
        // won't be needed
        let temp_dir = tempdir().unwrap();
        let project = isolated_project(temp_dir.path()).await;

        let global_specs = specs
            .to_global_specs(&channel_config, &manifest_root, &project, &[])
            .await
            .unwrap();

        assert_eq!(global_specs.len(), 1);
        assert!(matches!(
            &global_specs.first().unwrap().spec,
            &PixiSpec::Git(..)
        ))
    }

    #[tokio::test]
    async fn test_to_global_specs_with_conda_path_copies_file() {
        let temp_dir = tempdir().unwrap();
        let pixi_home_dir = temp_dir.path().join("pixi-home");
        let file_name = "custom-package.conda".to_string();
        let source_path = temp_dir.path().join(&file_name);
        fs::write(&source_path, b"dummy package").unwrap();

        temp_env::async_with_vars([("PIXI_HOME", Some(pixi_home_dir.to_str().unwrap()))], {
            let pixi_home_dir = pixi_home_dir.clone();
            let source_path = source_path.clone();
            let file_name = file_name.clone();
            async move {
                fs::create_dir_all(&pixi_home_dir).unwrap();

                let project = pixi_global::Project::discover_or_create().await.unwrap();
                let channel_config = project.global_channel_config().clone();
                let manifest_root = project.root.clone();

                let specs = GlobalSpecs {
                    specs: vec!["custom-package".to_string()],
                    path: Some(Utf8NativePathBuf::from(
                        source_path.to_string_lossy().to_string(),
                    )),
                    ..Default::default()
                };

                let global_specs = specs
                    .to_global_specs(&channel_config, &manifest_root, &project, &[])
                    .await
                    .unwrap();

                assert_eq!(global_specs.len(), 1);
                let installed_spec = &global_specs[0];

                let PixiSpec::PathBinary(path_spec) = &installed_spec.spec else {
                    panic!("expected binary path spec");
                };

                let resolved_path = path_spec
                    .resolve(&manifest_root)
                    .unwrap()
                    .canonicalize()
                    .unwrap();
                let expected_destination = pixi_home_dir
                    .join("conda-files")
                    .join(&file_name)
                    .canonicalize()
                    .unwrap();

                assert_eq!(resolved_path, expected_destination);
                assert!(
                    expected_destination.is_file(),
                    "expected copied .conda file to exist"
                );
            }
        })
        .await;
    }

    #[tokio::test]
    async fn test_to_global_specs_nameless() {
        let specs = GlobalSpecs {
            specs: vec![">=1.0".to_string()],
            ..Default::default()
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let manifest_root = PathBuf::from(".");

        // Create a dummy project
        let temp_dir = tempdir().unwrap();
        let project = isolated_project(temp_dir.path()).await;

        let global_specs = specs
            .to_global_specs(&channel_config, &manifest_root, &project, &[])
            .await;
        assert!(global_specs.is_err());
    }

    /// `--build-backend NAME` is sugar for a `build.backend.name` entry in the
    /// inline package definition.
    #[test]
    fn test_build_backend_sugar() {
        let specs = GlobalSpecs {
            build_backend: Some("pixi-build-rust".to_string()),
            ..Default::default()
        };
        let inline = specs.inline_package_value().unwrap().unwrap();
        assert_eq!(
            inline.to_toml_value().to_string(),
            r#"{ build.backend.name = "pixi-build-rust" }"#
        );
    }

    /// A version constraint on `--build-backend` lands as `build.backend.version`.
    #[test]
    fn test_build_backend_with_version() {
        let specs = GlobalSpecs {
            build_backend: Some("pixi-build-rust>=0.3,<0.4".to_string()),
            ..Default::default()
        };
        let inline = specs.inline_package_value().unwrap().unwrap();
        assert_eq!(
            inline.to_toml_value().to_string(),
            r#"{ build.backend = { name = "pixi-build-rust", version = ">=0.3,<0.4" } }"#
        );
    }

    /// `--package` fragments merge into the definition.
    #[test]
    fn test_package_fragments_merge() {
        let specs = GlobalSpecs {
            build_backend: Some("pixi-build-python".to_string()),
            package: vec![
                "host-dependencies.hatchling=\"*\"".to_string(),
                "build.config.profile=\"release\"".to_string(),
            ],
            ..Default::default()
        };
        let inline = specs.inline_package_value().unwrap().unwrap();
        let rendered = inline.to_toml_value().to_string();
        assert!(
            rendered.contains(r#"name = "pixi-build-python""#),
            "{rendered}"
        );
        assert!(rendered.contains(r#"hatchling = "*""#), "{rendered}");
        assert!(rendered.contains(r#"profile = "release""#), "{rendered}");
    }

    /// Setting the same key via `--build-backend` and `--package` is an error.
    #[test]
    fn test_package_collision_with_build_backend() {
        let specs = GlobalSpecs {
            build_backend: Some("pixi-build-rust".to_string()),
            package: vec!["build.backend.name=\"other\"".to_string()],
            ..Default::default()
        };
        let err = specs.inline_package_value().unwrap_err();
        assert!(
            matches!(err, GlobalSpecsConversionError::PackageKeyCollision { .. }),
            "expected a collision error, got {err:?}"
        );
    }

    /// Neither flag means no inline definition.
    #[test]
    fn test_no_inline_definition() {
        let specs = GlobalSpecs::default();
        assert!(specs.inline_package_value().unwrap().is_none());
    }

    /// One inline definition given next to several named packages is attached
    /// to each of them.
    #[tokio::test]
    async fn test_inline_applies_to_all_named_packages() {
        let specs = GlobalSpecs {
            specs: vec!["foo".to_string(), "bar".to_string()],
            git: Some("https://github.com/user/repo.git".parse().unwrap()),
            build_backend: Some("pixi-build-rust".to_string()),
            ..Default::default()
        };
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let manifest_root = PathBuf::from(".");
        let project = pixi_global::Project::discover_or_create().await.unwrap();
        let global_specs = specs
            .to_global_specs(&channel_config, &manifest_root, &project, &[])
            .await
            .unwrap();
        assert_eq!(global_specs.len(), 2);
        assert!(
            global_specs.iter().all(|spec| spec.inline.is_some()),
            "every named package should carry the inline definition"
        );
    }

    /// `--build-backend`/`--package` without a source location is an error.
    #[tokio::test]
    async fn test_inline_requires_source() {
        let specs = GlobalSpecs {
            specs: vec!["xsv".to_string()],
            build_backend: Some("pixi-build-rust".to_string()),
            ..Default::default()
        };
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let manifest_root = PathBuf::from(".");
        let project = pixi_global::Project::discover_or_create().await.unwrap();
        let err = specs
            .to_global_specs(&channel_config, &manifest_root, &project, &[])
            .await
            .unwrap_err();
        assert!(
            matches!(err, GlobalSpecsConversionError::InlineRequiresSource),
            "expected an inline-requires-source error, got {err:?}"
        );
    }

    /// `--build-backend` accepts only a name and a version constraint; any other
    /// matchspec component (channel, build string, subdir, ...) is rejected.
    #[test]
    fn test_build_backend_rejects_non_version_matchspec() {
        for input in [
            "conda-forge::pixi-build-rust",
            "pixi-build-rust[build=foo]",
            "pixi-build-rust[subdir=noarch]",
        ] {
            let specs = GlobalSpecs {
                build_backend: Some(input.to_string()),
                ..Default::default()
            };
            let err = specs.inline_package_value().unwrap_err();
            assert!(
                matches!(err, GlobalSpecsConversionError::InvalidBuildBackend { .. }),
                "expected an invalid-build-backend error for '{input}', got {err:?}"
            );
        }
    }

    /// A `--package` fragment that isn't a `DOTTED_KEY=TOML_VALUE` pair is
    /// rejected. In particular, the right-hand side must be a valid TOML
    /// value: unquoted strings and malformed arrays error instead of being
    /// silently recorded as strings.
    #[test]
    fn test_invalid_package_fragment() {
        for fragment in [
            "no-equals-sign",
            "=novalue",
            "build.config.profile=release",
            "build.config.extra-args=[1,2",
        ] {
            let specs = GlobalSpecs {
                package: vec![fragment.to_string()],
                ..Default::default()
            };
            let err = specs.inline_package_value().unwrap_err();
            assert!(
                matches!(
                    err,
                    GlobalSpecsConversionError::InvalidPackageFragment { .. }
                ),
                "expected an invalid-package-fragment error for '{fragment}', got {err:?}"
            );
        }
    }

    /// Two `--package` fragments setting the same leaf key collide.
    #[test]
    fn test_two_package_fragments_collide() {
        let specs = GlobalSpecs {
            package: vec![
                "host-dependencies.hatchling=\"1\"".to_string(),
                "host-dependencies.hatchling=\"2\"".to_string(),
            ],
            ..Default::default()
        };
        let err = specs.inline_package_value().unwrap_err();
        assert!(
            matches!(err, GlobalSpecsConversionError::PackageKeyCollision { .. }),
            "expected a collision error, got {err:?}"
        );
    }

    /// `--build-backend NAME` records the same definition as
    /// `--package 'build.backend.name="NAME"'`.
    #[test]
    fn test_build_backend_equivalent_to_package() {
        let sugar = GlobalSpecs {
            build_backend: Some("pixi-build-rust".to_string()),
            ..Default::default()
        };
        let explicit = GlobalSpecs {
            package: vec!["build.backend.name=\"pixi-build-rust\"".to_string()],
            ..Default::default()
        };
        assert_eq!(
            sugar
                .inline_package_value()
                .unwrap()
                .unwrap()
                .to_toml_value()
                .to_string(),
            explicit
                .inline_package_value()
                .unwrap()
                .unwrap()
                .to_toml_value()
                .to_string(),
        );
    }

    /// The inline definition may not set `name` (it comes from the dependency
    /// key) nor `build.source` (it comes from the dependency spec). Both checks
    /// live past the `build.backend` requirement, so a backend must be present
    /// for the definition to parse far enough to reach them.
    #[test]
    fn test_inline_definition_rejects_reserved_keys() {
        let name = PackageName::new_unchecked("xsv");
        let root = std::path::Path::new(".");

        let explicit_name = GlobalSpecs {
            build_backend: Some("pixi-build-rust".to_string()),
            package: vec!["name=\"other\"".to_string()],
            ..Default::default()
        };
        let err = explicit_name
            .inline_package_value()
            .unwrap()
            .unwrap()
            .to_inline_manifest(&name, root)
            .unwrap_err();
        assert!(
            matches!(
                err,
                pixi_global::project::InlinePackageValueError::ExplicitName
            ),
            "expected an explicit-name error, got {err:?}"
        );

        let explicit_source = GlobalSpecs {
            build_backend: Some("pixi-build-rust".to_string()),
            package: vec!["build.source.path=\"elsewhere\"".to_string()],
            ..Default::default()
        };
        let err = explicit_source
            .inline_package_value()
            .unwrap()
            .unwrap()
            .to_inline_manifest(&name, root)
            .unwrap_err();
        assert!(
            matches!(
                err,
                pixi_global::project::InlinePackageValueError::ExplicitBuildSource
            ),
            "expected an explicit-build-source error, got {err:?}"
        );
    }
}
