use std::env;

use crate::global::{
    self, channel_name_from_prefix, install::sync_environment, print_executables_available, BinDir,
};
use clap::Parser;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pixi_config::{Config, ConfigCli};
use pixi_progress::wrap_in_progress;
use pixi_utils::{default_channel_config, reqwest::build_reqwest_clients};
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, PackageName, Platform};
use rattler_solve::{resolvo::Solver, SolverImpl, SolverTask};
use rattler_virtual_packages::VirtualPackage;

/// Sync global manifest with installed environments
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    config: ConfigCli,
}

/// Sync global manifest with installed environments
pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project = global::Project::discover()?.with_cli_config(config.clone());

    // Fetch the repodata
    let (_, auth_client) = build_reqwest_clients(Some(&config));

    let gateway = config.gateway(auth_client.clone());

    let bin_dir = BinDir::create().await?;

    let exposed_paths = project
        .environments()
        .values()
        .flat_map(|environment| {
            environment
                .exposed
                .keys()
                .map(|e| bin_dir.executable_script_path(e))
        })
        .collect_vec();

    for file in bin_dir.files().await? {
        if !exposed_paths.contains(&file) {
            tokio::fs::remove_file(&file)
                .await
                .into_diagnostic()
                .wrap_err_with(|| format!("Could not remove {}", &file.display()))?;
        }
    }

    for (environment_name, environment) in project.environments() {
        let specs = environment
            .dependencies
            .clone()
            .into_iter()
            .map(|(name, spec)| {
                let match_spec = MatchSpec::from_nameless(
                    spec.clone()
                        .try_into_nameless_match_spec(&default_channel_config())
                        .into_diagnostic()?
                        .ok_or_else(|| {
                            miette::miette!("Could not convert {spec:?} to nameless match spec.")
                        })?,
                    Some(name.clone()),
                );
                Ok((name, match_spec))
            })
            .collect::<Result<IndexMap<PackageName, MatchSpec>, miette::Report>>()?;

        let channels = environment
            .channels()
            .into_iter()
            .map(|channel| channel.clone().into_channel(config.global_channel_config()))
            .collect_vec();

        let repodata = gateway
            .query(
                channels,
                [environment.platform(), Platform::NoArch],
                specs.values().cloned().collect_vec(),
            )
            .recursive(true)
            .await
            .into_diagnostic()?;

        // Determine virtual packages of the current platform
        let virtual_packages = VirtualPackage::current()
            .into_diagnostic()
            .context("failed to determine virtual packages")?
            .iter()
            .cloned()
            .map(GenericVirtualPackage::from)
            .collect();

        // Solve the environment
        let solver_specs = specs.clone();
        let solved_records = wrap_in_progress("solving environment", move || {
            Solver.solve(SolverTask {
                specs: solver_specs.values().cloned().collect_vec(),
                virtual_packages,
                ..SolverTask::from_iter(&repodata)
            })
        })
        .into_diagnostic()
        .context("failed to solve environment")?;

        let packages = specs.keys().cloned().collect();

        sync_environment(
            &environment_name,
            &environment.exposed,
            packages,
            solved_records.clone(),
            auth_client.clone(),
            environment.platform(),
            &bin_dir,
        )
        .await?;

        // let mut executables = Vec::with_capacity(specs.len());
        // for (package_name, _) in specs {
        //     let record = &prefix_package.repodata_record.package_record;

        //     let channel_name =
        //         channel_name_from_prefix(&prefix_package, config.global_channel_config());
        //     eprintln!(
        //         "{}Installed package {} {} {} from {}",
        //         console::style(console::Emoji("✔ ", "")).green(),
        //         console::style(record.name.as_source()).bold(),
        //         console::style(record.version.version()).bold(),
        //         console::style(record.build.as_str()).bold(),
        //         channel_name,
        //     );
        //     executables.extend(scripts);
        // }

        // if !executables.is_empty() {
        //     print_executables_available(executables).await?;
        // }
    }

    Ok(())
}
