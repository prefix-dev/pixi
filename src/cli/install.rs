use std::str::FromStr;
use clap::Parser;
use rattler_conda_types::{Channel, ChannelConfig, Platform, MatchSpec};
use rattler_repodata_gateway::sparse::SparseRepoData;
use rattler_solve::{LibsolvRepoData, SolverBackend};
use crate::repodata::{fetch_sparse_repodata, friendly_channel_name};

/// Runs command in project.
#[derive(Parser, Debug)]
#[clap(trailing_var_arg = true)]
pub struct Args {
    /// Package to install
    package: String,

    /// Channel to install from
    #[clap(short, long, default_values = ["conda-forge"])]
    channels: Vec<String>,
}


pub async fn execute(args: Args) -> anyhow::Result<()> {
    let channels = args.channels.iter().map(|c| Channel::from_str(c, &ChannelConfig::default())).collect::<Result<Vec<Channel>, _>>()?;
    let package_matchspec = MatchSpec::from_str(&args.package).unwrap();
    let platform = Platform::current();

    println!("Installing: {}, from {}", package_matchspec, channels.iter().map(|c| friendly_channel_name(c)).collect::<Vec<_>>().join(", "));

    // Fetch sparse repodata
    let platform_sparse_repodata = fetch_sparse_repodata(&channels, &vec![platform]).await?;

    // Solve for environment
    let available_packages = SparseRepoData::load_records_recursive(
        platform_sparse_repodata.iter(),
        vec![args.package.clone()],
    )?;

    // Construct a solver task that we can start solving.
    let task = rattler_solve::SolverTask {
        specs: vec![package_matchspec],
        available_packages: available_packages
            .iter()
            .map(|records| LibsolvRepoData::from_records(records)),

        // TODO: All these things.
        locked_packages: vec![],
        pinned_packages: vec![],
        virtual_packages: vec![],
    };

    let records = rattler_solve::LibsolvBackend.solve(task)?;
    dbg!(records);


    Ok(())

}
