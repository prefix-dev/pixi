use clap::Parser;
use pixi_consts::consts;
use typed_path::TypedPathBuf;
use url::Url;

use crate::cli::has_specs::HasSpecs;

#[derive(Parser, Debug, Default, Clone)]
pub struct GlobalSpecs {
    /// The dependency as names, conda MatchSpecs
    #[arg(num_args = 1.., required = true, value_name = "PACKAGE")]
    pub specs: Vec<String>,

    /// The git url to use when adding a git dependency
    #[clap(long, short, help_heading = consts::CLAP_GIT_OPTIONS)]
    pub git: Option<Url>,

    #[clap(flatten)]
    /// The git revisions to use when adding a git dependency
    pub rev: Option<crate::cli::cli_config::GitRev>,

    /// The subdirectory of the git repository to use
    #[clap(long, short, requires = "git", help_heading = consts::CLAP_GIT_OPTIONS)]
    pub subdir: Option<String>,

    /// The path to the local directory to use when adding a local dependency
    #[clap(long, short, conflicts_with = "git")]
    pub path: Option<TypedPathBuf>,
}

impl HasSpecs for GlobalSpecs {
    fn packages(&self) -> Vec<&str> {
        self.specs.iter().map(AsRef::as_ref).collect()
    }
}
