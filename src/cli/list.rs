use std::path::PathBuf;

use clap::Parser;
use serde::Serialize;

use crate::prefix::Prefix;
use crate::Project;

/// List installed packages in the current environment
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    /// Whether to output in json format
    #[arg(long)]
    pub json: bool,

    /// Whether to output in pretty json format
    #[arg(long)]
    pub json_pretty: bool,

    /// Whether to sort the package list by name
    #[arg(long)]
    pub no_sort: bool,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

#[derive(Serialize)]
struct PackageToOutput {
    name: String,
    version: String,
    build: String,
    channel: String,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Load the project
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())
        .ok()
        .ok_or_else(|| miette::miette!("No project found"))?;

    // Load the prefix
    let prefix = Prefix::new(project.environment_dir())?;

    // Load the installed packages
    let prefix_records = prefix
        .find_installed_packages(None)
        .await
        .map_err(|_| miette::miette!("Cannot find installed packages"))?;

    let mut repodata_records = prefix_records
        .iter()
        .map(|p| &p.repodata_record)
        .collect::<Vec<_>>();

    // Sort packages by name if needed
    if !args.no_sort {
        // Sort packages by name
        repodata_records.sort_by(|a, b| {
            a.package_record
                .name
                .as_source()
                .cmp(b.package_record.name.as_source())
        });
    }

    // Convert the the list of package record to a hashmap so it's agnostic to the output logic.
    let packages_to_output = repodata_records
        .iter()
        .map(|p| {
            let channel = p.channel.split('/').collect::<Vec<_>>();
            let channel_name = channel[channel.len() - 1];

            let package_name = p.package_record.name.as_source();
            let version = p.package_record.version.as_str().clone();
            let build = p.package_record.build.as_str();

            PackageToOutput {
                name: package_name.to_string(),
                version: version.to_string(),
                build: build.to_string(),
                channel: channel_name.to_string(),
            }
        })
        .collect::<Vec<_>>();

    // Print as table string or JSON
    if args.json {
        // print packages as json
        json_packages(packages_to_output, args.json_pretty);
    } else {
        // print packages as table
        print_packages(packages_to_output);
    }

    Ok(())
}

fn print_packages(packages: Vec<PackageToOutput>) {
    println!(
        "{:40} {:19} {:19} {:19}",
        console::style("Package").bold(),
        console::style("Version").bold(),
        console::style("Build").bold(),
        console::style("Channel").bold(),
    );

    for package in packages {
        println!(
            "{:40} {:19} {:19} {:19}",
            console::style(package.name).cyan().bright(),
            console::style(package.version),
            console::style(package.build),
            console::style(package.channel),
        );
    }
}

fn json_packages(packages: Vec<PackageToOutput>, json_pretty: bool) {
    let json_string = if json_pretty {
        serde_json::to_string_pretty(&packages)
    } else {
        serde_json::to_string(&packages)
    }
    .map_err(|_| miette::miette!("Cannot serialize packages to JSON"))
    .unwrap();

    println!("{}", json_string);
}
