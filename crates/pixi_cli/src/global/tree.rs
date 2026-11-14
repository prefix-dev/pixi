use crate::cli_interface::cli_global_context;
use crate::shared::tree::{Dependency, Package, PackageSource, print_requested_tree};
use ahash::HashSet;
use clap::Parser;
use console::Color;
use itertools::Itertools;
use pixi_api::global::{EnvironmentName, Project};
use std::collections::HashMap;
use std::str::FromStr;

/// Show a tree of dependencies for a specific global environment.
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false, long_about = format!(
    "\
    Show a tree of a global environment dependencies\n\
    \n\
    Dependency names highlighted in {} are directly specified in the manifest.
    ",
    console::style("green").fg(Color::Green).bold(),
))]
pub struct Args {
    /// The environment to list packages for.
    #[arg(short, long)]
    pub environment: String,

    /// List only packages matching a regular expression
    #[arg()]
    pub regex: Option<String>,

    /// Invert tree and show what depends on a given package in the regex argument
    #[arg(short, long, requires = "regex")]
    pub invert: bool,

    /// Output the dependency tree as JSON
    #[arg(long)]
    pub json: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::discover_or_create().await?;
    let context = cli_global_context(project);
    let env_name = EnvironmentName::from_str(args.environment.as_str())?;
    let environment = context.get_environment(&env_name).await?;

    // Contains all the dependencies under conda-meta
    let packages: HashMap<String, Package> = environment
        .records
        .iter()
        .map(|record| {
            let name = record
                .repodata_record
                .package_record
                .name
                .as_normalized()
                .to_string();
            let package = Package {
                name: name.clone(),
                version: record
                    .repodata_record
                    .package_record
                    .version
                    .version()
                    .to_string()
                    .into(),
                dependencies: record
                    .repodata_record
                    .package_record
                    .as_ref()
                    .depends
                    .iter()
                    .filter_map(|dep| {
                        dep.split([' ', '='])
                            .next()
                            .map(|dep_name| dep_name.to_string())
                    })
                    .filter(|dep_name| !dep_name.starts_with("__")) // Filter virtual packages
                    .unique() // A package may be listed with multiple constraints
                    .map(|name| Dependency {
                        name,
                        via_extras: Vec::new(),
                    })
                    .collect(),
                needed_by: Vec::new(),
                source: PackageSource::Conda, // Global environments can only manage Conda packages
            };
            (name, package)
        })
        .collect();

    let direct_deps =
        HashSet::from_iter(environment.dependencies.iter().map(|dep| dep.name.clone()));
    print_requested_tree(
        &packages,
        &direct_deps,
        args.regex.as_deref(),
        args.invert,
        args.json,
    )
}
