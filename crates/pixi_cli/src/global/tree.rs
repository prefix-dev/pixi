use crate::shared::tree::{
    Package, PackageSource, build_reverse_dependency_map, print_dependency_tree,
    print_inverted_dependency_tree,
};
use ahash::HashSet;
use clap::Parser;
use console::Color;
use itertools::Itertools;
use miette::Context;
use pixi_consts::consts;
use pixi_global::common::find_package_records;
use pixi_global::{EnvRoot, EnvironmentName, Project};
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
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let project = Project::discover_or_create().await?;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let env_name = EnvironmentName::from_str(args.environment.as_str())?;
    let environment = project
        .environment(&env_name)
        .wrap_err("Environment not found")?;
    // Contains all the dependencies under conda-meta
    let records = find_package_records(
        &EnvRoot::from_env()
            .await?
            .path()
            .join(env_name.as_str())
            .join(consts::CONDA_META_DIR),
    )
    .await?;

    let packages: HashMap<String, Package> = records
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
                    .to_string(),
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
                    .collect(),
                needed_by: Vec::new(),
                source: PackageSource::Conda, // Global environments can only manage Conda packages
            };
            (name, package)
        })
        .collect();

    let direct_deps = HashSet::from_iter(
        environment
            .dependencies
            .specs
            .iter()
            .map(|(name, _)| name.as_normalized().to_string()),
    );
    if args.invert {
        print_inverted_dependency_tree(
            &mut handle,
            &build_reverse_dependency_map(&packages),
            &direct_deps,
            &args.regex,
        )
        .wrap_err("Couldn't print the inverted dependency tree")?;
    } else {
        print_dependency_tree(&mut handle, &packages, &direct_deps, &args.regex)
            .wrap_err("Couldn't print the dependency tree")?;
    }
    Ok(())
}
