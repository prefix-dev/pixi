use std::path::PathBuf;

use crate::{
    WorkspaceLocator,
    cli::cli_config::{LockFileUpdateConfig, WorkspaceConfig},
    lock_file::UpdateLockFileOptions,
};
use clap::Parser;
use miette::IntoDiagnostic;
use pixi_config::ConfigCli;
use rattler_lock::{Channel, DEFAULT_ENVIRONMENT_NAME, LockFile};

#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// Output directory for each split lock file.
    /// The paths of the output lock files are: {output_dir}/{platform}/{environment}.lock
    pub output_dir: PathBuf,

    /// Keep the original environment name in the output lock file instead of replacing by "default"
    #[arg(long, default_value = "false")]
    pub keep_env_name: bool,

    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(args.config.clone());

    let lockfile = workspace
        .update_lock_file(UpdateLockFileOptions {
            lock_file_usage: args.lock_file_update_config.lock_file_usage()?,
            no_install: args.lock_file_update_config.no_lockfile_update,
            max_concurrent_solves: workspace.config().max_concurrent_solves(),
        })
        .await?;

    split(lockfile.as_lock_file(), args)
}

fn split(lockfile: &LockFile, args: Args) -> miette::Result<()> {
    if lockfile.is_empty() {
        eprintln!("lockfile is empty.");
        return Ok(());
    }

    let mut split_lockfiles = Vec::new();

    for (env_name, env) in lockfile.environments() {
        let output_env_name = if args.keep_env_name {
            env_name
        } else {
            DEFAULT_ENVIRONMENT_NAME
        };
        for plat in env.platforms() {
            let mut builder = LockFile::builder()
                .with_channels(output_env_name, env.channels().iter().map(Channel::clone))
                .with_options(output_env_name, env.solve_options().clone());
            if let Some(pypi_indexes) = env.pypi_indexes() {
                builder.set_pypi_indexes(output_env_name, pypi_indexes.clone());
            }
            if let Some(packages) = env.packages(plat) {
                for p in packages {
                    builder.add_package(output_env_name, plat, p.into());
                }
            }
            let s = builder.finish();
            if s.is_empty() {
                tracing::warn!("Ignore empty environment {env_name} on platform {plat}");
            } else {
                split_lockfiles.push((env_name.to_string(), plat, s));
            }
        }
    }

    if split_lockfiles.is_empty() {
        eprintln!("No environments.");
    } else {
        fs_err::create_dir_all(&args.output_dir).into_diagnostic()?;
        for (env_name, plat, l) in split_lockfiles {
            let subdir = &args.output_dir.join(plat.to_string());
            fs_err::create_dir_all(subdir).into_diagnostic()?;
            l.to_path(subdir.join(format!("{}.lock", env_name)).as_path())
                .into_diagnostic()?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn test_split_lockfile() {
        let lockfile = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/mock-projects/test-project-export/pixi.lock");

        let lockfile = LockFile::from_path(&lockfile).unwrap();

        let output_dir = tempdir().unwrap();

        let args = Args {
            workspace_config: WorkspaceConfig::default(),
            output_dir: output_dir.path().to_path_buf(),
            keep_env_name: false,
            lock_file_update_config: LockFileUpdateConfig::default(),
            config: ConfigCli::default(),
        };

        split(&lockfile, args).unwrap();

        for (env_name, env) in lockfile.environments() {
            for plat in env.platforms() {
                let output = output_dir
                    .path()
                    .join(plat.to_string())
                    .join(format!("{}.lock", env_name));
                let output_lockfile = LockFile::from_path(output.as_path()).unwrap();
                let snapshot = format!("test_split_lockfile_{}_{}", env_name, plat);
                insta::assert_snapshot!(snapshot, output_lockfile.render_to_string().unwrap());
            }
        }
    }
}
