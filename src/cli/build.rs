use std::{collections::BTreeMap, path::PathBuf, str::FromStr};

use clap::Parser;
use miette::IntoDiagnostic;
use rattler_build::{
    build::run_build,
    metadata::{
        About, BuildOptions, Output, Package, PathSrc, RenderedRecipe, Requirements, ScriptEnv,
    },
    render::dependency_list::Dependency,
    tool_configuration::Configuration,
};
use rattler_conda_types::package::ArchiveType;
use rattler_conda_types::{MatchSpec, NoArchType, Platform};

use crate::{
    task::{CmdArgs, Task},
    Project,
};

/// Build the project into a conda package.
#[derive(Parser, Debug)]
pub struct Args {}

fn get_script(project: &Project) -> miette::Result<String> {
    // find the "install" script and all the dependencies

    let mut script: Vec<String> = Vec::new();

    let get_script = |x: &Task| match x {
        Task::Plain(cmd) => cmd.clone(),
        Task::Execute(cmd) => match &cmd.cmd {
            CmdArgs::Single(cmd) => cmd.to_string(),
            CmdArgs::Multiple(cmd_vec) => cmd_vec.join(" "),
        },
        Task::Alias(cmd) => cmd.depends_on.join(" && "),
    };

    let task = project
        .task_opt("install")
        .ok_or_else(|| miette::miette!("Could not find an install task"))?;

    script.push(get_script(task));

    let mut dependencies: Vec<String> = Vec::new();
    dependencies.extend(task.depends_on().iter().cloned());

    while !dependencies.is_empty() {
        let task = project
            .task_opt(&dependencies[0])
            .ok_or_else(|| miette::miette!("Could not find a task"))?;
        script.push(get_script(task));

        dependencies.extend(task.depends_on().iter().cloned());
        dependencies.remove(0);
    }

    script.reverse();
    Ok(script.join("\n"))
}

pub async fn execute(_args: Args) -> miette::Result<()> {
    let project = Project::discover()?;

    let directories = rattler_build::metadata::Directories::create(
        project.name(),
        project.root(),
        &PathBuf::from("target"),
    )
    .into_diagnostic()?;

    let channels = project.manifest.project.channels.clone();

    let target_platform = Platform::current();

    let build_configuration = rattler_build::metadata::BuildConfiguration {
        target_platform,
        host_platform: Platform::current(),
        build_platform: Platform::current(),
        variant: Default::default(),
        hash: "0".to_string(),
        no_clean: false,
        directories,
        channels: channels.iter().map(|c| c.canonical_name()).collect(),
        timestamp: chrono::Utc::now(),
        subpackages: Default::default(),
        package_format: ArchiveType::Conda,
    };

    let script = get_script(&project)?;

    tracing::info!("Assembled build script:\n{}", script);

    let host_dependencies: Vec<Dependency> = project
        .host_dependencies(target_platform)?
        .into_iter()
        .map(|dep| {
            let ms = MatchSpec::from_str(&format!("{} {}", dep.0.as_normalized(), dep.1)).unwrap();
            Dependency::Spec(ms)
        })
        .collect();

    let build_dependencies: Vec<Dependency> = project
        .build_dependencies(target_platform)?
        .into_iter()
        .map(|dep| {
            let ms = MatchSpec::from_str(&format!("{} {}", dep.0.as_normalized(), dep.1)).unwrap();
            Dependency::Spec(ms)
        })
        .collect();

    let dependencies: Vec<Dependency> = project
        .dependencies(target_platform)?
        .into_iter()
        .map(|dep| {
            let ms = MatchSpec::from_str(&format!("{} {}", dep.0.as_normalized(), dep.1)).unwrap();
            Dependency::Spec(ms)
        })
        .collect();

    let mut env_vars = BTreeMap::<String, String>::new();
    env_vars.insert(
        "PIXI_BUILD_FOLDER".to_string(),
        build_configuration
            .directories
            .work_dir
            .join("pixi-build")
            .to_string_lossy()
            .to_string(),
    );

    #[cfg(target_os = "windows")]
    let install_prefix = build_configuration.directories.host_prefix.join("Library");
    #[cfg(not(target_os = "windows"))]
    let install_prefix = build_configuration.directories.host_prefix.clone();

    env_vars.insert(
        "PIXI_INSTALL_PREFIX".to_string(),
        install_prefix.to_string_lossy().to_string(),
    );

    println!("Env vars: {:?}", env_vars);

    let recipe = RenderedRecipe {
        source: Some(vec![rattler_build::metadata::Source::Path(PathSrc {
            path: project.root().to_path_buf(),
            patches: Default::default(),
            folder: None,
        })]),
        build: BuildOptions {
            number: 0,
            // todo - how do we compute the build hash?
            string: Some("0".to_string()),
            script: Some(vec![script]),
            script_env: ScriptEnv {
                env: env_vars,
                secrets: Default::default(),
                passthrough: Default::default(),
            },
            ignore_run_exports: None,
            ignore_run_exports_from: None,
            run_exports: None,
            noarch: NoArchType::none(),
            entry_points: Vec::default(),
        },
        requirements: Requirements {
            build: build_dependencies,
            host: host_dependencies,
            run: dependencies,
            run_constrained: Vec::default(),
        },
        about: About {
            home: project.manifest.project.homepage.clone().map(|x| vec![x]),
            license: project.manifest.project.license.clone(),
            license_file: project
                .manifest
                .project
                .license_file
                .clone()
                .map(|x| vec![x.to_string_lossy().to_string()]),
            license_family: None,
            summary: project.manifest.project.description.clone(),
            // read README file
            description: project
                .manifest
                .project
                .readme
                .clone()
                .map(|x| std::fs::read_to_string(x).unwrap()),
            doc_url: project
                .manifest
                .project
                .documentation
                .clone()
                .map(|x| vec![x]),
            dev_url: project.manifest.project.repository.clone().map(|x| vec![x]),
        },
        package: Package {
            name: project.name().parse().into_diagnostic()?,
            version: project.version().to_string(),
        },
        test: None,
    };

    let output = Output {
        recipe,
        build_configuration,
        finalized_dependencies: None,
    };

    let configuration = Configuration::default();

    run_build(&output, configuration)
        .await
        .map_err(|e| miette::miette!(format!("Error building package: {}", e)))?;

    Ok(())
}
