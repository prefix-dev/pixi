use std::path::PathBuf;

use clap::Parser;
use comfy_table::presets::NOTHING;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};
use human_bytes::human_bytes;
use rattler_conda_types::Platform;
use rattler_lock::{LockedDependency, LockedDependencyKind};
use serde::Serialize;

use crate::lock_file::load_lock_file;
use crate::project::SpecType;
use crate::Project;

// an enum to sort by size or name
#[derive(clap::ValueEnum, Clone, Debug, Serialize)]
pub enum SortBy {
    Size,
    Name,
}

/// List project's packages. Highlighted packages are explicit dependencies.
#[derive(Debug, Parser)]
#[clap(arg_required_else_help = false)]
pub struct Args {
    /// List only packages matching a regular expression
    #[arg()]
    pub regex: Option<String>,

    /// The platform to list packages for. Defaults to the current platform.
    #[arg(long)]
    pub platform: Option<Platform>,

    /// Whether to output in json format
    #[arg(long)]
    pub json: bool,

    /// Whether to output in pretty json format
    #[arg(long)]
    pub json_pretty: bool,

    /// Sorting strategy
    #[arg(long, default_value = "name", value_enum)]
    pub sort_by: SortBy,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

#[derive(Serialize)]
struct PackageToOutput {
    name: String,
    version: String,
    build: Option<String>,
    size_bytes: Option<u64>,
    kind: String,
    channel: Option<String>,
    is_explicit: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Load the project
    let project = Project::load_or_else_discover(args.manifest_path.as_deref())
        .ok()
        .ok_or_else(|| miette::miette!("No project found"))?;

    // Load the platform
    let platform = args.platform.unwrap_or_else(Platform::current);

    // Load the environment
    // NOTE(hadim): make it an argument once environments are implemented.
    // let environment = project.default_environment();

    // Load the lockfile
    let lockfile = load_lock_file(&project)
        .await
        .map_err(|_| miette::miette!("Cannot load lockfile. Did you run `pixi install` first?"))?;

    let locked_deps = lockfile.packages_for_platform(platform).collect::<Vec<_>>();

    // Get the explicit project dependencies
    let project_dependency_names: Vec<String> = {
        let dependencies = project
            .default_environment()
            .dependencies(Some(SpecType::Run), Some(platform));
        dependencies
            .names()
            .map(|p| p.as_source().to_string())
            .collect()
    };

    // Convert the the list of package record to specific output format
    let mut packages_to_output = locked_deps
        .iter()
        .map(|p| create_package_to_output(p, &project_dependency_names))
        .collect::<Vec<PackageToOutput>>();

    // Filter packages by regex if needed
    if let Some(regex) = args.regex {
        let regex = regex::Regex::new(&regex).map_err(|_| miette::miette!("Invalid regex"))?;
        packages_to_output = packages_to_output
            .into_iter()
            .filter(|p| regex.is_match(&p.name))
            .collect::<Vec<_>>();
    }

    // Sort according to the sorting strategy
    match args.sort_by {
        SortBy::Size => {
            packages_to_output
                .sort_by(|a, b| a.size_bytes.unwrap_or(0).cmp(&b.size_bytes.unwrap_or(0)));
        }
        SortBy::Name => {
            packages_to_output.sort_by(|a, b| a.name.cmp(&b.name));
        }
    }

    // Print as table string or JSON
    if args.json || args.json_pretty {
        // print packages as json
        json_packages(packages_to_output, args.json_pretty);
    } else {
        // print packages as table
        print_packages_as_table(packages_to_output);
    }

    Ok(())
}

// fn print_packages(packages: Vec<PackageToOutput>) {
//     // NOTE(hadim): maybe switch to a third party package or a custom dynamic printing logic
//     // to adapt the table column width to the content of the table and the terminal.
//     // https://github.com/Nukesor/comfy-table/ could be a good candidate.

//     println!(
//         "{:40} {:14} {:19} {:12} {:6} {:19}",
//         console::style("Package").bold(),
//         console::style("Version").bold(),
//         console::style("Build").bold(),
//         console::style("Size").bold(),
//         console::style("Kind").bold(),
//         console::style("Channel").bold(),
//     );

//     for package in packages {
//         // Convert size to human readable format
//         let size_human = match package.size_bytes {
//             Some(size_bytes) => human_bytes(size_bytes as f64).to_string(),
//             None => "".to_string(),
//         };

//         println!(
//             "{:40} {:14} {:19} {:12} {:6} {:19}",
//             if package.is_explicit {
//                 console::style(package.name).green().bright().bold()
//             } else {
//                 console::style(package.name)
//             },
//             console::style(package.version),
//             console::style(package.build.unwrap_or_else(|| "".to_string())),
//             console::style(size_human),
//             console::style(package.kind),
//             console::style(package.channel.unwrap_or_else(|| "".to_string())),
//         );
//     }
// }

fn print_packages_as_table(packages: Vec<PackageToOutput>) {
    // Initialize table
    let mut table = Table::new();

    table
        .load_preset(NOTHING)
        // .apply_modifier(UTF8_NO_BORDERS)
        .set_content_arrangement(ContentArrangement::Dynamic);

    // Add headers
    table.set_header(vec![
        Cell::new("Package").add_attribute(Attribute::Bold),
        Cell::new("Version").add_attribute(Attribute::Bold),
        Cell::new("Build").add_attribute(Attribute::Bold),
        Cell::new("Size").add_attribute(Attribute::Bold),
        Cell::new("Kind").add_attribute(Attribute::Bold),
        Cell::new("Channel").add_attribute(Attribute::Bold),
    ]);

    for package in packages {
        // Convert size to human readable format
        let size_human = match package.size_bytes {
            Some(size_bytes) => human_bytes(size_bytes as f64).to_string(),
            None => "".to_string(),
        };

        let package_name = if package.is_explicit {
            Cell::new(package.name)
                .fg(Color::Green)
                .add_attribute(Attribute::Bold)
        } else {
            Cell::new(package.name)
        };

        table.add_row(vec![
            package_name,
            Cell::new(package.version),
            Cell::new(package.build.unwrap_or_else(|| "".to_string())),
            Cell::new(size_human),
            Cell::new(package.kind),
            Cell::new(package.channel.unwrap_or_else(|| "".to_string())),
        ]);
    }

    println!("{table}");
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

fn create_package_to_output(
    p: &LockedDependency,
    project_dependency_names: &[String],
) -> PackageToOutput {
    let name = p.name.clone();
    let version = p.version.clone();

    let kind = match p.kind {
        LockedDependencyKind::Conda(_) => "conda".to_string(),
        LockedDependencyKind::Pypi(_) => "pypi".to_string(),
    };
    let build = match p.kind {
        LockedDependencyKind::Conda(_) => p.as_conda().unwrap().build.clone(),
        LockedDependencyKind::Pypi(_) => p.as_pypi().unwrap().build.clone(),
    };

    let size_bytes = match p.kind {
        LockedDependencyKind::Conda(_) => p.as_conda().unwrap().size,
        LockedDependencyKind::Pypi(_) => None,
    };

    let channel = Some("".to_string());

    // NOTE(hadim): does not work - returns `conda-forge/osx-arm64/tk-8.6.13-h5083fa2_1.conda`
    // let channel = match p.kind {
    //     LockedDependencyKind::Conda(_) => {
    //         Channel::from_url(
    //             p.as_conda().unwrap().url.clone(),
    //             Some(vec![Platform::current()]),
    //             &ChannelConfig::default(),
    //         )
    //         .name
    //     }
    //     LockedDependencyKind::Pypi(_) => Some("".to_string()),
    // };

    let is_explicit = project_dependency_names.contains(&name);

    PackageToOutput {
        name,
        version,
        build,
        size_bytes,
        kind,
        channel,
        is_explicit,
    }
}
