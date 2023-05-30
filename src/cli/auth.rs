use clap::Parser;
use rattler_networking::Authentication;

use crate::auth::{default_authentication_storage, listen_and_open_browser};

/// Adds a dependency to the project
#[derive(Parser, Debug)]
pub struct Args {
    host: String,
    token: String,
}

pub async fn execute(args: Args) -> anyhow::Result<()> {
    let storage = default_authentication_storage()?;
    let auth = Authentication::BearerToken(args.token.to_string());
    storage.store(&args.host, &auth)?;

    listen_and_open_browser()?;

    Ok(())
}
