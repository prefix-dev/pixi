use crate::global::common::find_package_records;
use crate::global::project::ParsedEnvironment;
use crate::global::{EnvironmentName, ExposedName, Project};
use clap::Parser;
use fancy_display::FancyDisplay;
use human_bytes::human_bytes;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{miette, IntoDiagnostic};
use pixi_config::{Config, ConfigCli};
use pixi_consts::consts;
use pixi_spec::PixiSpec;
use rattler_conda_types::{PackageName, PackageRecord, PrefixRecord, Version};
use serde::Serialize;
use std::io::{stdout, Write};
use std::str::FromStr;
use thiserror::__private::AsDisplay;

/// Lists all packages previously installed into a globally accessible location via `pixi global install`.
///
/// All environments:
/// - Yellow: the binaries that are exposed.
/// - Green: the packages that are explicit dependencies of the environment.
/// - Blue: the version of the installed package.
/// - Cyan: the name of the environment.
///
/// Per environment:
/// - Green: packages are explicitly installed.
#[derive(Parser, Debug)]
pub struct Args {
    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,

    #[clap(flatten)]
    config: ConfigCli,

    /// The name of the environment to list.
    #[clap(short, long)]
    environment: Option<String>,

    /// Sorting strategy for the package table of an environment
    #[arg(long, default_value = "name", value_enum, requires = "environment")]
    sort_by: GlobalSortBy,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project = Project::discover_or_create(args.assume_yes)
        .await?
        .with_cli_config(config.clone());

    if let Some(environment) = args.environment {
        let name = EnvironmentName::from_str(environment.as_str())?;
        // Verify that the environment is in sync with the manifest and report to the user otherwise
        if !project.environment_in_sync(&name).await? {
            tracing::warn!("The environment '{}' is not in sync with the manifest, to sync run\n\tpixi global sync", name);
        }
        list_environment(project, &name, args.sort_by).await?;
    } else {
        // Verify that the environments are in sync with the manifest and report to the user otherwise
        if !project.environments_in_sync().await? {
            tracing::warn!("The environments are not in sync with the manifest, to sync run\n\tpixi global sync");
        }
        list_global_environments(project).await?;
    }

    Ok(())
}

/// Sorting strategy for the package table
#[derive(clap::ValueEnum, Clone, Debug, Serialize)]
pub enum GlobalSortBy {
    Size,
    Name,
}

#[derive(Serialize, Hash, Eq, PartialEq)]
struct PackageToOutput {
    name: PackageName,
    version: Version,
    build: Option<String>,
    size_bytes: Option<u64>,
    is_explicit: bool,
}

impl PackageToOutput {
    fn from_package_record(record: &PackageRecord, is_explicit: bool) -> Self {
        Self {
            name: record.name.clone(),
            version: record.version.version().clone(),
            build: Some(record.build.clone()),
            size_bytes: record.size,
            is_explicit,
        }
    }
}

/// List package and binaries in environment
async fn list_environment(
    project: Project,
    environment_name: &EnvironmentName,
    sort_by: GlobalSortBy,
) -> miette::Result<()> {
    let env = project
        .environments()
        .get(environment_name)
        .ok_or_else(|| miette!("Environment '{}' not found", environment_name))?;

    let records = find_package_records(
        &project
            .env_root
            .path()
            .join(environment_name.as_str())
            .join(consts::CONDA_META_DIR),
    )
    .await?;

    let mut packages_to_output: Vec<PackageToOutput> = records
        .iter()
        .map(|record| {
            PackageToOutput::from_package_record(
                &record.repodata_record.package_record,
                env.dependencies()
                    .contains_key(&record.repodata_record.package_record.name),
            )
        })
        .collect();

    // Sort according to the sorting strategy
    match sort_by {
        GlobalSortBy::Size => {
            packages_to_output
                .sort_by(|a, b| a.size_bytes.unwrap_or(0).cmp(&b.size_bytes.unwrap_or(0)));
        }
        GlobalSortBy::Name => {
            packages_to_output.sort_by(|a, b| a.name.cmp(&b.name));
        }
    }
    println!(
        "The '{}' environment has {} packages:",
        environment_name.fancy_display(),
        console::style(packages_to_output.len()).bold()
    );
    print_package_table(packages_to_output).into_diagnostic()?;
    println!();
    print_meta_info(env);

    Ok(())
}

fn print_meta_info(environment: &ParsedEnvironment) {
    // Print exposed binaries, if binary similar to path only print once.
    let formatted_exposed = environment
        .exposed
        .iter()
        .map(|(exp, path)| {
            if &exp.to_string() == path {
                exp.to_string()
            } else {
                format!("{} -> {}", exp, path)
            }
        })
        .join(", ");
    println!(
        "{}\n{}",
        console::style("Exposed:").bold().yellow(),
        formatted_exposed
    );

    // Print channels
    if !environment.channels().is_empty() {
        println!(
            "{}\n{}",
            console::style("Channels:").bold().yellow(),
            environment.channels().iter().join(", ")
        );
    }

    // Print platform
    if let Some(platform) = environment.platform() {
        println!(
            "{} {}",
            console::style("Platform:").bold().yellow(),
            platform
        );
    }
}

/// Create a human-readable representation of the global environment.
/// Using a tabwriter to align the columns.
fn print_package_table(packages: Vec<PackageToOutput>) -> Result<(), std::io::Error> {
    let mut writer = tabwriter::TabWriter::new(stdout());
    let header_style = console::Style::new().bold().yellow();
    let header = format!(
        "{}\t{}\t{}\t{}",
        header_style.apply_to("Package"),
        header_style.apply_to("Version"),
        header_style.apply_to("Build"),
        header_style.apply_to("Size"),
    );
    writeln!(writer, "{}", &header)?;

    for package in packages {
        // Convert size to human-readable format
        let size_human = package
            .size_bytes
            .map(|size| human_bytes(size as f64))
            .unwrap_or_default();

        let package_info = format!(
            "{}\t{}\t{}\t{}",
            package.name.as_normalized(),
            &package.version,
            package.build.as_deref().unwrap_or(""),
            size_human
        );

        writeln!(
            writer,
            "{}",
            if package.is_explicit {
                console::style(package_info).green().to_string()
            } else {
                package_info
            }
        )?;
    }

    writeln!(writer, "{}", header)?;

    writer.flush()
}

/// List all environments in the global environment
async fn list_global_environments(project: Project) -> miette::Result<()> {
    let envs = project.environments();

    let mut message = String::new();

    let len = envs.len();
    for (idx, (env_name, env)) in envs.into_iter().enumerate() {
        let env_dir = project.env_root.path().join(env_name.as_str());
        let records = find_package_records(&env_dir.join(consts::CONDA_META_DIR)).await?;

        let last = (idx + 1) == len;

        if last {
            message.push_str("└──");
        } else {
            message.push_str("├──");
        }

        if !env
            .dependencies()
            .iter()
            .any(|(pkg_name, _spec)| pkg_name.as_normalized() != env_name.as_str())
        {
            if let Some(env_package) = records.iter().find(|rec| {
                rec.repodata_record.package_record.name.as_normalized() == env_name.as_str()
            }) {
                message.push_str(&format!(
                    " {}: {}",
                    env_name.fancy_display(),
                    console::style(env_package.repodata_record.package_record.version.clone())
                        .blue()
                ));
            } else {
                message.push_str(&format!(" {}", env_name.fancy_display()));
            }
        } else {
            message.push_str(&format!(" {}", env_name.fancy_display()));
        }

        // Write dependencies
        if let Some(dep_message) = format_dependencies(
            env_name.as_str(),
            &env.dependencies,
            &records,
            last,
            !env.exposed.is_empty(),
        ) {
            message.push_str(&dep_message);
        }

        // Write exposed binaries
        if let Some(exp_message) = format_exposed(env_name.as_str(), env.exposed(), last) {
            message.push_str(&exp_message);
        }

        if !last {
            message.push('\n');
        }
    }
    if message.is_empty() {
        println!("No global environments found.");
    } else {
        println!(
            "Global environments at {}:\n{}",
            project
                .env_root
                .path()
                .parent()
                .unwrap_or(project.env_root.path())
                .as_display(),
            message
        );
    }

    Ok(())
}

/// Display a dependency in a human-readable format.
fn display_dependency(name: &PackageName, version: Option<Version>) -> String {
    if let Some(version) = version {
        format!(
            "{} {}",
            console::style(name.as_normalized()).green(),
            console::style(version).blue()
        )
    } else {
        console::style(name.as_normalized()).green().to_string()
    }
}

/// Creating the ASCII art representation of a section.
fn format_asciiart_section(label: &str, content: String, last: bool, more: bool) -> String {
    let prefix = if last { " " } else { "│" };
    let symbol = if more { "├" } else { "└" };
    format!("\n{}   {}─ {}: {}", prefix, symbol, label, content)
}

fn format_dependencies(
    env_name: &str,
    dependencies: &IndexMap<PackageName, PixiSpec>,
    records: &[PrefixRecord],
    last: bool,
    more: bool,
) -> Option<String> {
    if dependencies
        .iter()
        .any(|(pkg_name, _spec)| pkg_name.as_normalized() != env_name)
    {
        let content = dependencies
            .iter()
            .map(|(name, _spec)| {
                let version = records
                    .iter()
                    .find(|rec| {
                        rec.repodata_record.package_record.name.as_normalized()
                            == name.as_normalized()
                    })
                    .map(|rec| rec.repodata_record.package_record.version.version().clone());
                display_dependency(name, version)
            })
            .join(", ");
        Some(format_asciiart_section("dependencies", content, last, more))
    } else {
        None
    }
}

fn format_exposed(
    env_name: &str,
    exposed: &IndexMap<ExposedName, String>,
    last: bool,
) -> Option<String> {
    if exposed.iter().any(|(exp, _)| exp.to_string() != env_name) {
        let content = exposed
            .iter()
            .map(|(exp, _)| console::style(exp).yellow().to_string())
            .join(", ");
        Some(format_asciiart_section("exposes", content, last, false))
    } else {
        None
    }
}
