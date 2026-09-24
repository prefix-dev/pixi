//! Split local resolutions on disk, while exposing one lock file to existing consumers.
use std::path::PathBuf;

use indexmap::IndexMap;
use miette::{Context, IntoDiagnostic};
use pixi_manifest::{FeaturesExt, HasWorkspaceManifest, platform::host::host_subdir};
use pixi_record::{LockFileResolver, LockFileWriter};
use rattler_lock::{LockFile, LockedPackage, PlatformData};

use crate::{Workspace, workspace::Environment};

pub(super) fn validate(workspace: &Workspace) -> miette::Result<()> {
    if workspace.has_local_environments() {
        workspace
            .workspace_manifest()
            .workspace
            .local_platform()
            .into_diagnostic()?;
        for env in workspace
            .environments()
            .into_iter()
            .filter(FeaturesExt::is_lock_file_less)
        {
            if env.solve_group().is_some() {
                miette::bail!(
                    "lock-file-less environment '{}' cannot belong to a solve group",
                    env.name()
                );
            }
            if env.platforms().is_empty() {
                miette::bail!(
                    "lock-file-less environment '{}' cannot run on the host because of feature platform restrictions",
                    env.name()
                );
            }
        }
    }
    Ok(())
}

pub(super) fn format_upgrade_required() -> miette::Report {
    miette::miette!(
        "local resolutions require the current lock-file format; run `pixi lock` to explicitly upgrade and re-solve older locks first"
    )
}

pub(super) fn path(workspace: &Workspace, environment: &Environment<'_>) -> PathBuf {
    workspace
        .pixi_dir()
        .join("locks")
        .join(environment.name().as_str())
        .join(format!("{}.lock", host_subdir()))
}

/// Re-register source records as well as packages: raw copies retain package-table
/// indices from their original file and lose transitive build/host dependencies.
pub(super) fn combine(
    workspace: &Workspace,
    inputs: &[(&LockFile, Vec<String>)],
) -> miette::Result<LockFile> {
    let mut platforms = IndexMap::new();
    for (lock, names) in inputs {
        for name in names {
            if let Some(env) = lock.environment(name) {
                for (platform, _) in env.packages_by_platform() {
                    platforms.insert(
                        platform.name().clone(),
                        PlatformData {
                            name: platform.name().clone(),
                            subdir: platform.subdir(),
                            virtual_packages: platform.virtual_packages().to_vec(),
                        },
                    );
                }
            }
        }
    }
    let mut builder = LockFile::builder()
        .with_platforms(platforms.into_values().collect())
        .into_diagnostic()?;
    let mut writer = LockFileWriter::new(&mut builder);
    for (lock, names) in inputs {
        let resolver = LockFileResolver::build(lock, workspace.root()).into_diagnostic()?;
        for name in names {
            let Some(env) = lock.environment(name) else {
                continue;
            };
            writer
                .builder
                .set_channels(name, env.channels().iter().cloned());
            writer
                .builder
                .set_options(name, env.solve_options().clone());
            if let Some(indexes) = env.pypi_indexes() {
                writer.builder.set_pypi_indexes(name, indexes.clone());
            }
            for (platform, packages) in env.packages_by_platform() {
                for package in packages {
                    match package {
                        LockedPackage::Conda(_) => {
                            let record = resolver.get_for_package(package).ok_or_else(|| {
                                miette::miette!("could not resolve locked conda package")
                            })?;
                            let data =
                                record.into_conda_package_data(&mut writer, workspace.root());
                            writer
                                .builder
                                .add_conda_package(name, platform.name().as_str(), data)
                                .into_diagnostic()?;
                        }
                        LockedPackage::Pypi(_) => {
                            writer
                                .builder
                                .add_package(name, platform.name().as_str(), package.clone())
                                .into_diagnostic()?;
                        }
                    }
                }
            }
        }
    }
    drop(writer);
    Ok(builder.finish())
}

pub(super) fn write(
    workspace: &Workspace,
    lock: &LockFile,
    allow_format_upgrade: bool,
) -> miette::Result<()> {
    validate(workspace)?;
    // Other callers (add/remove/update) construct derived data directly and may
    // have discarded load metadata. Check every destination before writing any
    // file so those paths cannot accidentally upgrade a legacy resolution.
    if !allow_format_upgrade {
        let paths = std::iter::once(workspace.lock_file_path()).chain(
            workspace
                .environments()
                .into_iter()
                .filter(FeaturesExt::is_lock_file_less)
                .map(|env| path(workspace, &env)),
        );
        for path in paths.filter(|path| path.is_file()) {
            if LockFile::from_path(&path).into_diagnostic()?.version()
                < rattler_lock::FileFormatVersion::LATEST
            {
                return Err(format_upgrade_required());
            }
        }
    }
    let mut shared = Vec::new();
    for env in workspace.environments() {
        let name = env.name().as_str().to_owned();
        if env.is_lock_file_less() {
            let path = path(workspace, &env);
            fs_err::create_dir_all(path.parent().expect("local lock has a parent"))
                .into_diagnostic()?;
            combine(workspace, &[(lock, vec![name])])?
                .to_path(&path)
                .into_diagnostic()
                .context("failed to write local lock file")?;
        } else {
            shared.push(name);
        }
    }
    let path = workspace.lock_file_path();
    if !shared.is_empty() {
        combine(workspace, &[(lock, shared)])?
            .to_path(&path)
            .into_diagnostic()
            .context("failed to write shared lock file")?;
    } else if path.exists() {
        fs_err::remove_file(path)
            .into_diagnostic()
            .context("failed to remove obsolete shared lock file")?;
    }
    Ok(())
}
