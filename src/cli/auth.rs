use crate::project::Project;
use clap::Parser;
use rattler_conda_types::{version_spec::VersionOperator, MatchSpec, Version, VersionSpec};
use std::collections::HashMap;
use std::ops::Deref;
use rattler_auth::Authentication;
/// Adds a dependency to the project
#[derive(Parser, Debug)]
pub struct Args {
    host: String, 
    token: String,
}

pub async fn execute(mut args: Args) -> anyhow::Result<()> {

    let auth = Authentication::BearerToken(args.token.to_string());
    rattler_auth::store_authentication_entry(&args.host, &auth)?;

    Ok(())
}
