use fancy_display::FancyDisplay;
use indexmap::IndexSet;
use miette::{IntoDiagnostic, miette};
use pixi_consts::consts;
use pixi_global::{
    EnvironmentName, Project,
    common::{self, find_package_records},
    project::ParsedEnvironment,
};
use rattler_conda_types::{NamedChannelOrUrl, PackageName, PrefixRecord, Subdir};
use serde::Serialize;

/// An executable of a global environment exposed under a name.
#[derive(Debug, Clone, Serialize)]
pub struct GlobalExposedMapping {
    pub exposed_name: String,
    pub executable: String,
}

/// A dependency of a global environment with its installed version.
#[derive(Debug, Clone, Serialize)]
pub struct GlobalDependency {
    pub name: String,
    pub version: Option<String>,
}

/// The state of a global environment
#[derive(Debug, Clone, Serialize)]
pub struct GlobalEnvironment {
    pub name: EnvironmentName,
    pub version: Option<String>,
    pub dependencies: Vec<GlobalDependency>,
    pub exposed: Vec<GlobalExposedMapping>,
    pub shortcuts: IndexSet<PackageName>,
    pub channels: Vec<NamedChannelOrUrl>,
    pub platform: Option<Subdir>,
    pub records: Vec<PrefixRecord>,
}

/// Collect the state of one environment
async fn environment(
    project: &Project,
    env_name: &EnvironmentName,
    env: &ParsedEnvironment,
) -> miette::Result<GlobalEnvironment> {
    let conda_meta = project
        .env_root_path()
        .join(env_name.as_str())
        .join(consts::CONDA_META_DIR);
    let records = find_package_records(&conda_meta).await?;

    let version = if common::is_single_package_environment(project, env_name) {
        common::installed_version(project, env_name).await?
    } else {
        None
    };

    let dependencies = env
        .dependencies
        .specs
        .keys()
        .map(|name| {
            let version = records
                .iter()
                .find(|rec| {
                    rec.repodata_record.package_record.name.as_normalized() == name.as_normalized()
                })
                .map(|rec| {
                    rec.repodata_record
                        .package_record
                        .version
                        .version()
                        .to_string()
                });
            GlobalDependency {
                name: name.as_normalized().to_string(),
                version,
            }
        })
        .collect();

    Ok(GlobalEnvironment {
        name: env_name.clone(),
        version,
        dependencies,
        exposed: env
            .exposed
            .iter()
            .map(|mapping| GlobalExposedMapping {
                exposed_name: mapping.exposed_name().to_string(),
                executable: mapping.executable_relname().to_string(),
            })
            .collect(),
        shortcuts: env.shortcuts.clone().unwrap_or_default(),
        channels: env.channels().into_iter().cloned().collect(),
        platform: env.platform,
        records,
    })
}

/// Get a specific global environment
pub async fn get(
    project: &Project,
    environment_name: &EnvironmentName,
) -> miette::Result<GlobalEnvironment> {
    let env = project
        .environments()
        .get(environment_name)
        .ok_or_else(|| miette!("Environment {} not found", environment_name.fancy_display()))?;

    environment(project, environment_name, env).await
}

/// List all environments in the global environment, sorted by name
pub async fn list(
    project: &Project,
    regex: Option<String>,
) -> miette::Result<Vec<GlobalEnvironment>> {
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

    let mut environments = Vec::with_capacity(project_envs.len());
    for (env_name, env) in project_envs.iter() {
        environments.push(environment(project, env_name, env).await?);
    }

    Ok(environments)
}
