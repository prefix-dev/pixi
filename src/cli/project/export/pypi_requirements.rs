use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use miette::{Context, IntoDiagnostic};

use crate::cli::cli_config::PrefixUpdateConfig;
use crate::lock_file::UpdateLockFileOptions;
use crate::Project;
use rattler_conda_types::Platform;
use rattler_lock::{Environment, Package, PackageHashes, PypiPackage, PypiPackageData, UrlOrPath};

#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    /// Output directory for rendered requirements files
    pub output_dir: PathBuf,

    /// Environment to render. Can be repeated for multiple envs. Defaults to all environments
    #[arg(short, long)]
    pub environment: Option<Vec<String>>,

    /// The platform to render. Can be repeated for multiple platforms.
    /// Defaults to all platforms available for selected environments.
    #[arg(short, long)]
    pub platform: Option<Vec<Platform>>,

    /// Create a separate requirements.txt for dependencies that do not have an associated hash
    #[arg(long, default_value = "true")]
    pub split_reqs_no_hashes: bool,

    #[clap(flatten)]
    pub prefix_update_config: PrefixUpdateConfig,
}

#[derive(Debug)]
struct PypiPackageReqData {
    source: String,
    hash_flag: Option<String>,
    editable: bool,
}

impl PypiPackageReqData {
    fn from_pypi_package(p: &PypiPackage) -> Self {
        // pip --verify-hashes does not accept hashes for local files
        let (s, include_hash) = match p.url() {
            UrlOrPath::Url(url) => (url.as_str(), true),
            UrlOrPath::Path(path) => (
                path.as_os_str()
                    .to_str()
                    .unwrap_or_else(|| panic!("Could not convert {:?} to str", path)),
                false,
            ),
        };
        //
        // remove "direct+ since not valid for pip urls"
        let s = s.trim_start_matches("direct+");

        let hash_flag = if include_hash {
            get_pypi_hash_str(p.data().package)
        } else {
            None
        };

        Self {
            source: s.to_string(),
            hash_flag,
            editable: p.is_editable(),
        }
    }

    fn to_req_entry(&self) -> String {
        let mut entry = String::new();

        if self.editable {
            entry.push_str("-e ");
        }
        entry.push_str(&self.source);

        if let Some(hash) = &self.hash_flag {
            entry.push_str(&format!(" {}", hash));
        }

        entry
    }
}

fn get_pypi_hash_str(package_data: &PypiPackageData) -> Option<String> {
    if let Some(hashes) = &package_data.hash {
        let h = match hashes {
            PackageHashes::Sha256(h) => format!("--hash=sha256:{:x}", h).to_string(),
            PackageHashes::Md5Sha256(_, h) => format!("--hash=sha256:{:x}", h).to_string(),
            PackageHashes::Md5(h) => format!("--hash=md5:{:x}", h).to_string(),
        };
        Some(h)
    } else {
        None
    }
}

fn render_pypi_requirements(
    target: impl AsRef<Path>,
    packages: &[PypiPackageReqData],
) -> miette::Result<()> {
    if packages.is_empty() {
        return Ok(());
    }

    let target = target.as_ref();

    let reqs = packages
        .iter()
        .map(|p| p.to_req_entry())
        .collect::<Vec<_>>()
        .join("\n");

    fs::write(target, reqs)
        .into_diagnostic()
        .with_context(|| format!("failed to write requirements file: {}", target.display()))?;

    Ok(())
}

fn render_env_platform(
    output_dir: &Path,
    env_name: &str,
    env: &Environment,
    platform: &Platform,
    split_nohash: bool,
) -> miette::Result<()> {
    let packages = env.packages(*platform).ok_or(miette::miette!(
        "platform '{platform}' not found for env {}",
        env_name,
    ))?;

    let mut pypi_packages: Vec<PypiPackageReqData> = Vec::new();

    for package in packages {
        match package {
            Package::Pypi(p) => pypi_packages.push(PypiPackageReqData::from_pypi_package(&p)),
            Package::Conda(cp) => {
                tracing::warn!(
                    "ignoring Conda package {} since Conda packages are not supported in requirements.txt",
                    cp.package_record().name.as_normalized()
                );
            }
        }
    }

    let (base, nohash) = if split_nohash {
        // Split package list based on presence of hash since pip currently treats requirements files
        // containing any hashes as if `--require-hashes` has been supplied. The only known workaround
        // is to split the dependencies, which are typically from vcs sources into a separate
        // requirements.txt and to install it separately.
        pypi_packages
            .into_iter()
            .partition(|p| p.editable || p.hash_flag.is_some())
    } else {
        (pypi_packages, vec![])
    };

    tracing::info!("Creating requirements file for env: {env_name} platform: {platform}");
    let target = output_dir
        .join(format!("{}_{}_requirements.txt", env_name, platform))
        .into_os_string();

    render_pypi_requirements(target, &base)?;

    if !nohash.is_empty() {
        tracing::info!(
            "Creating secondary requirements file for env: {env_name} platform: {platform} \
            containing  packages without hashes. This file will have to be installed separately."
        );
        let target = output_dir
            .join(format!("{}_{}_requirements_nohash.txt", env_name, platform))
            .into_os_string();

        render_pypi_requirements(target, &nohash)?;
    }

    Ok(())
}

pub async fn execute(project: Project, args: Args) -> miette::Result<()> {
    let lockfile = project
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: args.prefix_update_config.lock_file_usage(),
            no_install: args.prefix_update_config.no_install,
            ..UpdateLockFileOptions::default()
        })
        .await?
        .lock_file;

    let mut environments = Vec::new();
    if let Some(env_names) = args.environment {
        for env_name in &env_names {
            environments.push((
                env_name.to_string(),
                lockfile
                    .environment(env_name)
                    .ok_or(miette::miette!("unknown environment {}", env_name))?,
            ));
        }
    } else {
        for (env_name, env) in lockfile.environments() {
            environments.push((env_name.to_string(), env));
        }
    };

    let mut env_platform = Vec::new();

    for (env_name, env) in environments {
        let available_platforms: HashSet<Platform> = HashSet::from_iter(env.platforms());

        if let Some(ref platforms) = args.platform {
            for plat in platforms {
                if available_platforms.contains(plat) {
                    env_platform.push((env_name.clone(), env.clone(), *plat));
                } else {
                    tracing::warn!(
                        "Platform {} not available for environment {}. Skipping...",
                        plat,
                        env_name,
                    );
                }
            }
        } else {
            for plat in available_platforms {
                env_platform.push((env_name.clone(), env.clone(), plat));
            }
        }
    }

    fs::create_dir_all(&args.output_dir).ok();

    for (env_name, env, plat) in env_platform {
        render_env_platform(
            &args.output_dir,
            &env_name,
            &env,
            &plat,
            args.split_reqs_no_hashes,
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use rattler_lock::LockFile;
    use tempfile::tempdir;

    #[test]
    fn test_render_pypi_requirements() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/cli/project/export/test-data/testenv-pypi/pixi.lock");
        let lockfile = LockFile::from_path(&path).unwrap();

        let output_dir = tempdir().unwrap();

        for (env_name, env) in lockfile.environments() {
            for platform in env.platforms() {
                render_env_platform(output_dir.path(), env_name, &env, &platform, true).unwrap();

                let file_path = output_dir
                    .path()
                    .join(format!("{}_{}_requirements.txt", env_name, platform));
                insta::assert_snapshot!(
                    format!("test_render_pypi_requirements_{}_{}", env_name, platform),
                    fs::read_to_string(file_path).unwrap()
                );

                let file_path = output_dir
                    .path()
                    .join(format!("{}_{}_requirements_nohash.txt", env_name, platform));
                insta::assert_snapshot!(
                    format!(
                        "test_render_pypi_requirements_nohash_{}_{}",
                        env_name, platform
                    ),
                    fs::read_to_string(file_path).unwrap()
                );
            }
        }
    }

    #[test]
    fn test_render_pypi_requirements_nosplit() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/cli/project/export/test-data/testenv-pypi/pixi.lock");
        let lockfile = LockFile::from_path(&path).unwrap();

        let output_dir = tempdir().unwrap();

        for (env_name, env) in lockfile.environments() {
            for platform in env.platforms() {
                render_env_platform(output_dir.path(), env_name, &env, &platform, false).unwrap();

                let file_path = output_dir
                    .path()
                    .join(format!("{}_{}_requirements.txt", env_name, platform));
                insta::assert_snapshot!(
                    format!(
                        "test_render_pypi_requirements_nosplit_{}_{}",
                        env_name, platform
                    ),
                    fs::read_to_string(file_path).unwrap()
                );

                // Check to make sure no "nohash" file is created
                let file_path = output_dir
                    .path()
                    .join(format!("{}_{}_requirements_nohash.txt", env_name, platform));
                assert!(!file_path.exists());
            }
        }
    }
}
