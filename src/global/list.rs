use std::io::stdout;

use fancy_display::FancyDisplay;
use human_bytes::human_bytes;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use pixi_consts::consts;
use pixi_spec::PixiSpec;
use rattler_conda_types::{PackageName, PackageRecord, PrefixRecord, Version};
use serde::Serialize;
use std::io::Write;

use miette::{miette, IntoDiagnostic};

use crate::global::common::find_package_records;

use super::{project::ParsedEnvironment, EnvChanges, EnvState, EnvironmentName, Mapping, Project};

/// Sorting strategy for the package table
#[derive(clap::ValueEnum, Clone, Debug, Serialize, Default)]
pub enum GlobalSortBy {
    Size,
    #[default]
    Name,
}

/// Creating the ASCII art representation of a section.
pub fn format_asciiart_section(label: &str, content: String, last: bool, more: bool) -> String {
    let prefix = if last { " " } else { "│" };
    let symbol = if more { "├" } else { "└" };
    format!("\n{}   {}─ {}: {}", prefix, symbol, label, content)
}

pub fn format_exposed(exposed: &IndexSet<Mapping>, last: bool) -> Option<String> {
    if exposed.is_empty() {
        Some(format_asciiart_section(
            "exposes",
            console::style("Nothing").dim().red().to_string(),
            last,
            false,
        ))
    } else {
        let formatted_exposed = exposed.iter().map(format_mapping).join(", ");
        Some(format_asciiart_section(
            "exposes",
            formatted_exposed,
            last,
            false,
        ))
    }
}

fn format_mapping(mapping: &Mapping) -> String {
    let exp = mapping.exposed_name().to_string();
    if exp == mapping.executable_relname() {
        console::style(exp).yellow().to_string()
    } else {
        format!(
            "{} -> {}",
            console::style(exp).yellow(),
            console::style(mapping.executable_relname()).yellow()
        )
    }
}

fn print_meta_info(environment: &ParsedEnvironment) {
    // Print exposed binaries, if binary similar to path only print once.
    let formatted_exposed = environment.exposed.iter().map(format_mapping).join(", ");
    println!(
        "{}\n{}",
        console::style("Exposes:").bold().cyan(),
        if !formatted_exposed.is_empty() {
            formatted_exposed
        } else {
            "Nothing".to_string()
        }
    );

    // Print channels
    if !environment.channels().is_empty() {
        println!(
            "{}\n{}",
            console::style("Channels:").bold().cyan(),
            environment.channels().iter().join(", ")
        );
    }

    // Print platform
    if let Some(platform) = environment.platform() {
        println!("{} {}", console::style("Platform:").bold().cyan(), platform);
    }
}

/// Create a human-readable representation of the global environment.
/// Using a tabwriter to align the columns.
fn print_package_table(packages: Vec<PackageToOutput>) -> Result<(), std::io::Error> {
    let mut writer = tabwriter::TabWriter::new(stdout());
    let header_style = console::Style::new().bold().cyan();
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

/// List package and binaries in environment
pub async fn list_environment(
    project: &Project,
    environment_name: &EnvironmentName,
    sort_by: GlobalSortBy,
    regex: Option<String>,
) -> miette::Result<()> {
    let env = project
        .environments()
        .get(environment_name)
        .ok_or_else(|| miette!("Environment {} not found", environment_name.fancy_display()))?;

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
            PackageToOutput::new(
                &record.repodata_record.package_record,
                env.dependencies()
                    .contains_key(&record.repodata_record.package_record.name),
            )
        })
        .collect();

    // Filter according to the regex
    if let Some(ref regex) = regex {
        let regex = regex::Regex::new(regex).into_diagnostic()?;
        packages_to_output.retain(|package| regex.is_match(package.name.as_normalized()));
    }

    let output_message = if let Some(ref regex) = regex {
        format!(
            "The {} environment has {} packages filtered by regex `{}`:",
            environment_name.fancy_display(),
            console::style(packages_to_output.len()).bold(),
            regex
        )
    } else {
        format!(
            "The {} environment has {} packages:",
            environment_name.fancy_display(),
            console::style(packages_to_output.len()).bold()
        )
    };

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
    println!("{}", output_message);
    print_package_table(packages_to_output).into_diagnostic()?;
    println!();
    print_meta_info(env);

    Ok(())
}

/// List all environments in the global environment
pub async fn list_global_environments(
    project: &Project,
    envs: Option<Vec<EnvironmentName>>,
    envs_changes: Option<&EnvChanges>,
    regex: Option<String>,
) -> miette::Result<()> {
    let mut project_envs = project.environments().clone();
    project_envs.sort_by(|a, _, b, _| a.to_string().cmp(&b.to_string()));

    if let Some(regex) = regex {
        let regex = regex::Regex::new(&regex).into_diagnostic()?;
        project_envs.retain(|env_name, _| regex.is_match(env_name.as_str()));
    }

    if let Some(envs) = envs {
        project_envs.retain(|env_name, _| envs.contains(env_name));
    }

    let mut message = String::new();

    let len = project_envs.len();
    for (idx, (env_name, env)) in project_envs.iter().enumerate() {
        let env_dir = project.env_root.path().join(env_name.as_str());
        let records = find_package_records(&env_dir.join(consts::CONDA_META_DIR)).await?;

        let last = (idx + 1) == len;

        if last {
            message.push_str("└──");
        } else {
            message.push_str("├──");
        }

        // get the state of the environment if available
        // and also it's state if present
        let state = envs_changes
            .and_then(|env_changes| env_changes.changes.get(env_name))
            .map(|state| match state {
                EnvState::Installed => {
                    format!("({})", console::style("installed".to_string()).green())
                }
                EnvState::NotChanged(ref reason) => {
                    format!("({})", reason.fancy_display())
                }
            })
            .unwrap_or("".to_string());

        if !env
            .dependencies()
            .iter()
            .any(|(pkg_name, _spec)| pkg_name.as_normalized() != env_name.as_str())
        {
            if let Some(env_package) = records.iter().find(|rec| {
                rec.repodata_record.package_record.name.as_normalized() == env_name.as_str()
            }) {
                // output the environment name and version
                message.push_str(&format!(
                    " {}: {} {}",
                    env_name.fancy_display(),
                    console::style(env_package.repodata_record.package_record.version.clone())
                        .blue(),
                    state
                ));
            } else {
                message.push_str(&format!(" {} {}", env_name.fancy_display(), state));
            }
        } else {
            message.push_str(&format!(" {} {}", env_name.fancy_display(), state));
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
        if let Some(exp_message) = format_exposed(env.exposed(), last) {
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
            "Global environments as specified in '{}'\n{}",
            console::style(project.manifest.path.display()).bold(),
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

#[derive(Serialize, Hash, Eq, PartialEq)]
struct PackageToOutput {
    name: PackageName,
    version: Version,
    build: Option<String>,
    size_bytes: Option<u64>,
    is_explicit: bool,
}

impl PackageToOutput {
    fn new(record: &PackageRecord, is_explicit: bool) -> Self {
        Self {
            name: record.name.clone(),
            version: record.version.version().clone(),
            build: Some(record.build.clone()),
            size_bytes: record.size,
            is_explicit,
        }
    }
}
