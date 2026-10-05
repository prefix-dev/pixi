use std::{
    collections::HashSet,
    io::IsTerminal,
    path::{Path, PathBuf},
};

use clap::Parser;
use miette::{Context, IntoDiagnostic};
use pep508_rs::Requirement;
use pixi_api::workspace::{DependencyOptions, GitOptions};
use pixi_config::ConfigCli;
use pixi_consts::consts;
use pixi_core::{
    DependencyType, Workspace, WorkspaceLocator,
    environment::LockFileUsage,
    workspace::{DiscoveryStart, PypiDeps, SkippedPackage},
};
use pixi_manifest::{
    HasFeaturesIter, HasWorkspaceManifest, KnownPreviewFlag, PypiDependencyLocation,
};
use pixi_pypi_spec::{PixiPypiSource, PixiPypiSpec, PypiPackageName};
use url::Url;

use crate::{
    cli_config::{DependencyConfig, LockFileUpdateConfig, NoInstallConfig, ScriptWorkspaceConfig},
    cli_interface::{CliInterface, cli_context},
    has_specs::HasSpecs,
};

/// Adds dependencies to the workspace
///
/// The dependencies should be defined as MatchSpec for conda package, a PyPI
/// requirement for the `--pypi` dependencies, or an absolute path to a local
/// `.conda` or `.tar.bz2` package file. If no specific version is provided,
/// the latest version compatible with your workspace will be chosen
/// automatically or a * will be used.
///
/// Example usage:
///
/// - `pixi add python=3.9`: This will select the latest minor version that
///   complies with 3.9.*, i.e., python version 3.9.0, 3.9.1, 3.9.2, etc.
/// - `pixi add python`: In absence of a specified version, the latest version
///   will be chosen. For instance, this could resolve to python version
///   3.11.3.* at the time of writing.
///
/// Adding multiple dependencies at once is also supported:
///
/// - `pixi add python pytest`: This will add both `python` and `pytest` to the
///   workspace's dependencies.
///
/// The `--platform` and `--build/--host` flags make the dependency target
/// specific.
///
/// - `pixi add python --platform linux-64 --platform osx-arm64`: Will add the
///   latest version of python for linux-64 and osx-arm64 platforms.
/// - `pixi add python --build`: Will add the latest version of python for as a
///   build dependency.
///
/// Mixing `--platform` and `--build`/`--host` flags is supported
///
/// The `--pypi` option will add the package as a pypi dependency. This cannot
/// be mixed with the conda dependencies
///
/// - `pixi add --pypi boto3`
/// - `pixi add --pypi "boto3==version"`
///
/// Local path dependencies can be added with `--path`. Paths are resolved from
/// the current directory and stored relative to the workspace manifest, so an
/// absolute input path does not become machine-specific manifest data.
///
/// - `pixi add mylib --path ../mylib`
/// - `pixi add mylib --path /absolute/path/to/mylib`
/// - `pixi add recipe-package --path ../recipe/recipe.yaml`
/// - `pixi add --pypi mylib --path ../mylib`
/// - `pixi add --pypi mylib --path ../mylib --editable`
///
/// If the workspace manifest is a `pyproject.toml`, adding a pypi dependency will
/// add it to the native pyproject `project.dependencies` array or to the native
/// `dependency-groups` table if a feature is specified:
///
/// - `pixi add --pypi boto3` will add `boto3` to the `project.dependencies`
///   array
/// - `pixi add --pypi boto3 --feature aws` will add `boto3` to the
///   `dependency-groups.aws` array
/// - `pixi add --pypi --editable 'boto3 @ file://absolute/path/to/boto3'` will add
///   the local editable `boto3` to the `pypi-dependencies` array
///
/// Note that if `--platform` or `--editable` are specified, the pypi dependency
/// will be added to the `tool.pixi.pypi-dependencies` table instead as native
/// arrays have no support for platform-specific or editable dependencies.
///
/// These dependencies will then be read by pixi as if they had been added to
/// the pixi `pypi-dependencies` tables of the default or of a named feature.
///
/// The versions will be automatically added with a pinning strategy based on
/// semver or the pinning strategy set in the config. There is a list of
/// packages that are not following the semver versioning scheme but will use
/// the minor version by default:
/// Python, Rust, Julia, GCC, GXX, GFortran, NodeJS, Deno, R, R-Base, Perl
#[derive(Parser, Debug, Default)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: ScriptWorkspaceConfig,

    #[clap(flatten)]
    pub dependency_config: DependencyConfig,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,

    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    pub config: ConfigCli,

    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    /// The local path to use when adding a path dependency.
    #[arg(long, conflicts_with = "git")]
    pub path: Option<PathBuf>,

    /// Whether the pypi requirement should be editable
    #[arg(long, requires = "pypi")]
    pub editable: bool,

    /// The PyPI index URL to use for this dependency.
    /// Only applicable when adding pypi dependencies.
    #[clap(long, requires = "pypi", conflicts_with_all = ["git", "path"])]
    pub index: Option<Url>,
}

impl Args {
    fn validate_script_options(&self) -> miette::Result<()> {
        if self.workspace_config.script.is_none() {
            return Ok(());
        }

        let mut unsupported = Vec::new();
        if !self.dependency_config.feature.is_default() {
            unsupported.push("--feature");
        }
        if self.dependency_config.environment.is_some() {
            unsupported.push("--environment");
        }
        if self.dependency_config.host {
            unsupported.push("--host");
        }
        if self.dependency_config.build {
            unsupported.push("--build");
        }

        if unsupported.is_empty() {
            Ok(())
        } else {
            Err(miette::miette!(
                help = "A script has one implicit default run environment.",
                "`pixi add --script` does not support {}",
                unsupported.join(", ")
            ))
        }
    }

    fn dependency_options(&self, workspace: &Workspace) -> miette::Result<DependencyOptions> {
        Ok(DependencyOptions {
            feature: self.dependency_config.feature_name(),
            platforms: self.dependency_config.platforms.clone(),
            no_install: self.no_install_config.no_install,
            lock_file_usage: add_lock_file_usage(
                self.lock_file_update_config.lock_file_usage()?,
                self.workspace_config.script.is_some(),
                workspace.lock_file_path().is_file(),
            ),
        })
    }
}

impl From<&Args> for GitOptions {
    fn from(args: &Args) -> Self {
        GitOptions {
            git: args.dependency_config.git.clone(),
            path: args.path.clone(),
            reference: args
                .dependency_config
                .rev
                .clone()
                .unwrap_or_default()
                .into(),
            subdir: args.dependency_config.subdirectory(),
        }
    }
}

fn map_pypi_requirements_with_index(
    requirements: impl Iterator<Item = (PypiPackageName, Requirement)>,
    index: Option<&Url>,
) -> miette::Result<PypiDeps> {
    requirements
        .map(|(name, req)| {
            let pixi_spec = if let Some(index_url) = index {
                // Create spec from requirement
                let mut spec = PixiPypiSpec::try_from(req.clone())
                    .map_err(|e| miette::miette!("failed to convert requirement: {}", e))?;

                // Only apply index if this is a Registry source
                match spec.source_mut() {
                    PixiPypiSource::Registry { index, .. } => {
                        *index = Some(index_url.clone());
                        Some(spec) // Return spec only when index was actually applied
                    }
                    _ => None, // For Git, Path, etc. - index doesn't apply
                }
            } else {
                None // No index provided
            };

            Ok((name, (req, pixi_spec, None)))
        })
        .collect()
}

fn path_error(path: &Path, message: impl std::fmt::Display) -> miette::Report {
    miette::miette!("invalid dependency path '{}': {message}", path.display())
}

/// Resolve a CLI path from the current directory and rewrite it relative to the
/// workspace manifest. Also verifies that the target is a supported package
/// manifest (or a directory containing one).
pub(crate) fn resolve_dependency_path(
    path: &Path,
    workspace: &Workspace,
    pypi: bool,
) -> miette::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().into_diagnostic()?.join(path)
    };
    let absolute = dunce::canonicalize(&absolute)
        .into_diagnostic()
        .wrap_err_with(|| format!("dependency path '{}' does not exist", path.display()))?;

    let manifest = if absolute.is_dir() {
        if pypi {
            let pyproject = absolute.join("pyproject.toml");
            if !pyproject.is_file() {
                return Err(path_error(
                    path,
                    "directory does not contain pyproject.toml",
                ));
            }
            pyproject
        } else {
            let pixi = absolute.join("pixi.toml");
            let pyproject = absolute.join("pyproject.toml");
            if pixi.is_file() {
                pixi
            } else if pyproject.is_file() {
                pyproject
            } else {
                return Err(path_error(
                    path,
                    "directory does not contain pixi.toml or pyproject.toml",
                ));
            }
        }
    } else {
        let file_name = absolute.file_name().and_then(|name| name.to_str());
        let supported = if pypi {
            file_name == Some("pyproject.toml")
        } else {
            matches!(
                file_name,
                Some("pixi.toml" | "pyproject.toml" | "recipe.yaml" | "recipe.yml" | "package.xml")
            )
        };
        if !supported {
            return Err(path_error(path, "not a supported package manifest"));
        }
        absolute.clone()
    };

    if !pypi
        && matches!(
            manifest.file_name().and_then(|name| name.to_str()),
            Some("pixi.toml" | "pyproject.toml")
        )
    {
        let contents = fs_err::read_to_string(&manifest).into_diagnostic()?;
        let document = contents
            .parse::<toml_edit::DocumentMut>()
            .into_diagnostic()?;
        let has_package =
            if manifest.file_name().and_then(|name| name.to_str()) == Some("pixi.toml") {
                document.get("package").is_some()
            } else {
                document
                    .get("tool")
                    .and_then(|tool| tool.get("pixi"))
                    .and_then(|pixi| pixi.get("package"))
                    .is_some()
            };
        if !has_package {
            if manifest.file_name().and_then(|name| name.to_str()) == Some("pyproject.toml") {
                return Err(miette::miette!(
                    help = "Use `pixi add --pypi NAME --path PATH` for a Python path dependency",
                    "'{}' is a Python project, not a pixi-build package source",
                    manifest.display()
                ));
            }
            return Err(miette::miette!(
                help = "Add a `[package]` section to the package manifest",
                "'{}' is a pixi workspace, not a pixi-build package source",
                manifest.display()
            ));
        }
    }

    let dependency_path = match manifest.file_name().and_then(|name| name.to_str()) {
        Some("pixi.toml" | "pyproject.toml") => manifest
            .parent()
            .expect("a canonical manifest path has a parent directory"),
        _ => &manifest,
    };
    let manifest_dir = workspace
        .workspace
        .provenance
        .path
        .parent()
        .ok_or_else(|| miette::miette!("workspace manifest has no parent directory"))?;
    pathdiff::diff_paths(dependency_path, manifest_dir).ok_or_else(|| {
        miette::miette!(
            "could not make dependency path '{}' relative to workspace manifest '{}'",
            dependency_path.display(),
            workspace.workspace.provenance.path.display()
        )
    })
}

pub(crate) fn manifest_path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) async fn ensure_pixi_build_preview_enabled(
    workspace: Workspace,
    config_source: &pixi_config::ConfigSourceCli,
    search_start: DiscoveryStart,
    config: ConfigCli,
) -> miette::Result<Workspace> {
    if (&workspace)
        .workspace_manifest()
        .workspace
        .preview
        .is_enabled(KnownPreviewFlag::PixiBuild)
    {
        return Ok(workspace);
    }

    let hint = "Run `pixi workspace preview add pixi-build` to enable the preview flag";
    let enable = std::io::stdin().is_terminal()
        && dialoguer::Confirm::new()
            .with_prompt("Conda source dependencies require the pixi-build preview feature. Enable pixi-build for this workspace?")
            .default(false)
            .interact()
            .into_diagnostic()?;
    if !enable {
        return Err(miette::miette!(
            help = hint,
            "conda source dependencies are not allowed without enabling the 'pixi-build' preview flag"
        ));
    }

    pixi_api::WorkspaceContext::add_preview_flags(
        CliInterface {},
        workspace.workspace.provenance.path.clone(),
        vec![KnownPreviewFlag::PixiBuild],
    )
    .await?;

    Ok(WorkspaceLocator::for_cli()
        .with_global_config_source(config_source.source())
        .with_search_start(search_start)
        .with_cli_config(config)
        .locate()?)
}

fn pypi_path_deps(package: &str, path: &Path, workspace: &Workspace) -> miette::Result<PypiDeps> {
    let requirement_text = format!("{package} @ {}", manifest_path_string(path));
    let requirement = Requirement::parse(&requirement_text, workspace.root()).into_diagnostic()?;
    let name =
        PypiPackageName::from_normalized(requirement.name.clone()).with_source(package.to_string());
    let spec = PixiPypiSpec::try_from(requirement.clone())
        .map_err(|error| miette::miette!("failed to create path requirement: {error}"))?;
    Ok([(
        name,
        (
            requirement,
            Some(spec),
            Some(PypiDependencyLocation::PixiPypiDependencies),
        ),
    )]
    .into_iter()
    .collect())
}

pub async fn execute(args: Args) -> miette::Result<()> {
    args.validate_script_options()?;
    args.dependency_config.warn_deprecated_subdir();

    let mut workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .with_cli_config(args.config.clone())
        .locate()?;

    // Apply backend override if provided (primarily for testing)
    if let Some(backend_override) = args
        .workspace_config
        .workspace_config
        .backend_override
        .clone()
    {
        workspace = workspace.with_backend_override(backend_override);
    }

    let resolved_path = if let Some(path) = &args.path {
        if args.dependency_config.specs.len() != 1 {
            return Err(miette::miette!("--path requires exactly one package name"));
        }
        Some(resolve_dependency_path(
            path,
            &workspace,
            args.dependency_config.pypi,
        )?)
    } else {
        None
    };

    if resolved_path.is_some() && !args.dependency_config.pypi {
        workspace = ensure_pixi_build_preview_enabled(
            workspace,
            &args.config_source,
            args.workspace_config.workspace_locator_start(),
            args.config.clone(),
        )
        .await?;
        if let Some(backend_override) = args
            .workspace_config
            .workspace_config
            .backend_override
            .clone()
        {
            workspace = workspace.with_backend_override(backend_override);
        }
    }

    if workspace.is_conda_script() && !args.dependency_config.platforms.is_empty() {
        return Err(miette::miette!(
            help = "restrict the dependency with a `when` condition in `[dependencies]`, or write `[tool.pixi.target.<platform>.dependencies]` by hand",
            "`--platform` cannot edit a conda-script block"
        ));
    }

    if workspace.is_conda_script() && args.dependency_config.git.is_some() {
        return Err(miette::miette!(
            help = "the specification keeps `[dependencies]` to registry packages; add the git spec under `[tool.pixi.dependencies]` by editing the block",
            "conda-script `[dependencies]` do not support git specs"
        ));
    }

    let workspace_ctx = cli_context(workspace.clone());

    let (update_deps, skipped, parsed_names): (_, Vec<SkippedPackage>, Vec<String>) =
        match args.dependency_config.dependency_type() {
            DependencyType::CondaDependency(spec_type) => {
                let git_options = GitOptions {
                    git: args.dependency_config.git.clone(),
                    path: resolved_path.clone(),
                    reference: args
                        .dependency_config
                        .rev
                        .clone()
                        .unwrap_or_default()
                        .into(),
                    subdir: args.dependency_config.subdirectory(),
                };

                let specs = args.dependency_config.specs()?;
                let names: Vec<String> = specs
                    .keys()
                    .map(|n| n.as_normalized().to_string())
                    .collect();
                let result = workspace_ctx
                    .add_conda_deps(
                        specs,
                        spec_type,
                        args.dependency_options(&workspace)?,
                        git_options,
                    )
                    .await?;
                (result.0, result.1, names)
            }
            DependencyType::PypiDependency => {
                let pypi_deps = if let Some(path) = &resolved_path {
                    pypi_path_deps(&args.dependency_config.specs[0], path, &workspace)?
                } else {
                    let requirements_iter = match args
                        .dependency_config
                        .vcs_pep508_requirements(&workspace)
                        .transpose()?
                    {
                        Some(vcs_reqs) => vcs_reqs.into_iter(),
                        None => args.dependency_config.pypi_deps(&workspace)?.into_iter(),
                    };
                    map_pypi_requirements_with_index(requirements_iter, args.index.as_ref())?
                };

                let names: Vec<String> = pypi_deps
                    .keys()
                    .map(|n| n.as_normalized().to_string())
                    .collect();
                let result = workspace_ctx
                    .add_pypi_deps(
                        pypi_deps,
                        args.editable,
                        args.dependency_options(&workspace)?,
                    )
                    .await?;
                (result.0, result.1, names)
            }
        };

    let skipped_set: HashSet<&str> = skipped.iter().map(|s| s.name.as_str()).collect();

    for package in &skipped {
        let name = &package.name;
        if package.inherits_workspace {
            eprintln!(
                "{}{} inherits from `[workspace.dependencies]` and was left unchanged",
                console::style(console::Emoji("✔ ", "")).green(),
                console::style(name).bold(),
            );
            eprintln!(
                "  Update the `[workspace.dependencies]` entry, or pass an explicit spec like `{}` to replace the inheritance",
                console::style(format!("pixi add \"{name}==1.0\""))
                    .green()
                    .bold(),
            );
        } else {
            eprintln!(
                "{}{} is already a dependency",
                console::style(console::Emoji("✔ ", "")).green(),
                console::style(name).bold(),
            );
            eprintln!(
                "  Run `{}` to get the newest compatible version",
                console::style(format!("pixi upgrade {name}"))
                    .green()
                    .bold(),
            );
        }
    }

    let added_specs: Vec<String> = args
        .dependency_config
        .specs
        .iter()
        .zip(parsed_names.iter())
        .filter(|(_, name)| !skipped_set.contains(name.as_str()))
        .map(|(raw, _)| raw.clone())
        .collect();
    let display_config = DependencyConfig {
        specs: added_specs,
        ..args.dependency_config
    };
    display_config.display_success(
        "Added",
        update_deps
            .map(|update| update.implicit_constraints)
            .unwrap_or_default(),
    );

    // A feature that is not part of any environment is never solved, so
    // the added dependencies could not be resolved or pinned. Point the
    // user at the missing steps instead of leaving the `*` unexplained.
    // Re-running the same `pixi add` would hit the "already a dependency"
    // path, so `pixi upgrade` is the command that replaces the `*`.
    let added_names = parsed_names
        .iter()
        .filter(|name| !skipped_set.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let feature_name = display_config.feature_name();
    if let Some(feature) = feature_name.non_default()
        && !feature_name.is_environment()
        && !added_names.is_empty()
        && !workspace
            .environments()
            .iter()
            .any(|env| env.features().any(|f| f.name == feature_name))
    {
        eprintln!(
            "{}The feature {} is not used in any environment, so the added dependencies were not resolved or pinned",
            console::style(console::Emoji("⚠️ ", "warning: ")).yellow(),
            consts::FEATURE_STYLE.apply_to(feature),
        );
        eprintln!(
            "  Add it to an environment with `{}`",
            console::style(format!(
                "pixi workspace environment add <environment> --feature {feature}"
            ))
            .green()
            .bold(),
        );
        eprintln!(
            "  Then run `{}` to resolve and pin the dependencies",
            console::style(format!("pixi upgrade --feature {feature} {added_names}"))
                .green()
                .bold(),
        );
    }

    Ok(())
}

fn add_lock_file_usage(
    requested: LockFileUsage,
    is_script: bool,
    lock_file_exists: bool,
) -> LockFileUsage {
    if is_script && !lock_file_exists && requested == LockFileUsage::Update {
        LockFileUsage::DryRun
    } else {
        requested
    }
}
