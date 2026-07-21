use std::path::PathBuf;

use clap::Parser;

use crate::cli_config::{DependencyConfig, LockFileUpdateConfig, NoInstallConfig};

/// Add dependencies to a script's inline metadata.
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

    /// Add the requirements as portable PyPI dependencies.
    #[arg(long)]
    pub pypi: bool,

    /// Script to update.
    pub path: PathBuf,

    /// Conda MatchSpecs or, with `--pypi`, PEP 508 requirements.
    #[arg(required = true, value_name = "SPEC")]
    pub specs: Vec<String>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    crate::add::execute(crate::add::Args {
        script: Some(args.path),
        dependency_config: DependencyConfig {
            specs: args.specs,
            pypi: args.pypi,
            ..Default::default()
        },
        no_install_config: args.no_install_config,
        lock_file_update_config: args.lock_file_update_config,
        config: args.config,
        config_source: args.config_source,
        ..Default::default()
    })
    .await
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Args;

    #[test]
    fn rejects_workspace_only_options() {
        assert!(
            Args::try_parse_from(["add", "--feature", "test", "example.py", "pytest"]).is_err()
        );
        assert!(
            Args::try_parse_from(["add", "--platform", "linux-64", "example.py", "python"])
                .is_err()
        );
        assert!(
            Args::try_parse_from(["add", "--editable", "--pypi", "example.py", "foo"]).is_err()
        );
    }
}
