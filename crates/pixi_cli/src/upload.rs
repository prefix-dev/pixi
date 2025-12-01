use clap::Parser;
use miette::IntoDiagnostic;
use rattler_networking::AuthenticationStorage;
use rattler_upload::upload::opt::UploadOpts;

/// Upload conda packages to various channels
///
/// Supported server types: prefix, anaconda, quetz, artifactory, s3, conda-forge
///
/// Use `pixi auth login` to authenticate with the server.
#[derive(Parser, Debug)]
pub struct Args {
    #[command(flatten)]
    pub upload_opts: UploadOpts,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Get authentication storage from pixi's auth system
    let auth_storage = AuthenticationStorage::from_env_and_defaults().into_diagnostic()?;

    let opts = args.upload_opts.with_auth_store(Some(auth_storage));

    rattler_upload::upload_from_args(opts).await
}
