use std::path::PathBuf;

use clap::Parser;

use crate::cli_config::{DependencyConfig, LockFileUpdateConfig, NoInstallConfig};

/// Remove dependencies from a script's inline metadata.
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,

    #[clap(flatten)]
    pub config: pixi_config::ConfigCli,

    /// Remove the requirements as PyPI dependencies.
    #[arg(long)]
    pub pypi: bool,

    /// Script to update.
    pub path: PathBuf,

    /// Dependency names to remove.
    #[arg(required = true, value_name = "NAME")]
    pub names: Vec<String>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    crate::remove::execute(crate::remove::Args {
        config_source: args.config_source,
        script: Some(args.path),
        dependency_config: DependencyConfig {
            specs: args.names,
            pypi: args.pypi,
            ..Default::default()
        },
        no_install_config: args.no_install_config,
        lock_file_update_config: args.lock_file_update_config,
        config: args.config,
        ..Default::default()
    })
    .await
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Args;

    #[test]
    fn accepts_conda_and_explicit_pypi_names() {
        let args = Args::try_parse_from(["remove", "example.py", "requests"]).unwrap();
        assert!(!args.pypi);
        assert_eq!(args.names, ["requests"]);

        let args = Args::try_parse_from(["remove", "--pypi", "example.py", "requests"]).unwrap();
        assert!(args.pypi);
        assert_eq!(args.names, ["requests"]);
    }

    #[test]
    fn rejects_workspace_only_options() {
        assert!(
            Args::try_parse_from(["remove", "--feature", "test", "example.py", "pytest"]).is_err()
        );
        assert!(
            Args::try_parse_from(["remove", "--platform", "linux-64", "example.py", "python"])
                .is_err()
        );
    }
}
