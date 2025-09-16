use fancy_display::FancyDisplay;
use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use miette::{IntoDiagnostic, miette};
use pixi_consts::consts;
use pixi_core::environment::list::{PackageToOutput, print_package_table};
use pixi_spec::PixiSpec;
use rattler_conda_types::{PackageName, PrefixRecord, Version};

use super::{EnvChanges, EnvState, EnvironmentName, Mapping, Project, project::ParsedEnvironment};
use crate::common::find_package_records;

/// Creating the ASCII art representation of a section.
pub fn format_asciiart_section(label: &str, content: String, last: bool, more: bool) -> String {
    let prefix = if last { " " } else { "│" };
    let symbol = if more { "├" } else { "└" };
    format!("\n{}   {}─ {}: {}", prefix, symbol, label, content)
}

pub fn format_exposed(exposed: &IndexSet<Mapping>, last: bool, more: bool) -> Option<String> {
    if exposed.is_empty() {
        Some(format_asciiart_section(
            "exposes",
            console::style("Nothing").dim().red().to_string(),
            last,
            more,
        ))
    } else {
        let formatted_exposed = exposed.iter().map(format_mapping).join(", ");
        Some(format_asciiart_section(
            "exposes",
            formatted_exposed,
            last,
            more,
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
    if let Some(platform) = environment.platform {
        println!("{} {}", console::style("Platform:").bold().cyan(), platform);
    }
}

/// List package and binaries in global environment
pub async fn list_specific_global_environment(
    project: &Project,
    environment_name: &EnvironmentName,
    sort_by_size: bool,
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

    let mut packages_to_output = records
        .iter()
        .map(|record| {
            PackageToOutput::new(
                &record.repodata_record.package_record,
                env.dependencies
                    .specs
                    .contains_key(&record.repodata_record.package_record.name),
            )
        })
        .collect_vec();

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
    if sort_by_size {
        packages_to_output
            .sort_by(|a, b| a.size_bytes.unwrap_or(0).cmp(&b.size_bytes.unwrap_or(0)));
    } else {
        packages_to_output.sort_by(|a, b| a.name.cmp(&b.name));
    }
    println!("{}", output_message);
    print_package_table(packages_to_output).into_diagnostic()?;
    print_meta_info(env);

    Ok(())
}

/// List all environments in the global environment
pub async fn list_all_global_environments(
    project: &Project,
    envs: Option<Vec<EnvironmentName>>,
    envs_changes: Option<&EnvChanges>,
    regex: Option<String>,
    show_header: bool,
) -> miette::Result<()> {
    let mut project_envs = project.environments().clone();
    project_envs.sort_by(|a, _, b, _| a.to_string().cmp(&b.to_string()));

    project_envs.retain(|env_name, parsed_environment| {
        if parsed_environment.dependencies.specs.is_empty() {
            tracing::warn!(
                "Environment {} doesn't contain dependencies. Skipping.",
                env_name.fancy_display()
            );
            false
        } else {
            true
        }
    });

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
        let conda_meta = env_dir.join(consts::CONDA_META_DIR);
        let records = find_package_records(&conda_meta).await?;

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
                EnvState::NotChanged(reason) => {
                    format!("({})", reason.fancy_display())
                }
            })
            .unwrap_or_default();

        if !env
            .dependencies
            .specs
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
            &env.dependencies.specs,
            &records,
            last,
            !env.exposed.is_empty(),
        ) {
            message.push_str(&dep_message);
        }

        // Check for shortcuts
        let shortcuts = env.shortcuts.clone().unwrap_or_else(IndexSet::new);

        // Write exposed binaries
        if let Some(exp_message) = format_exposed(&env.exposed, last, !shortcuts.is_empty()) {
            message.push_str(&exp_message);
        }

        // Write shortcuts
        if !shortcuts.is_empty() {
            message.push_str(&format_asciiart_section(
                "shortcuts",
                shortcuts.iter().map(PackageName::as_normalized).join(", "),
                last,
                false,
            ));
        }

        if !last {
            message.push('\n');
        }
    }
    if message.is_empty() {
        println!("No global environments found.");
    } else {
        let header = format!(
            "Global environments as specified in '{}'",
            project.manifest.path.display()
        );
        if show_header {
            println!("{}", header);
        }
        println!("{}", message);
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
