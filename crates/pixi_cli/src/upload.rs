use clap::Parser;
use miette::IntoDiagnostic;
use pixi_auth::get_auth_store;
use pixi_config::Config;
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
    // Get authentication storage using pixi's auth system which respects
    // the authentication_override_file configuration
    let config = Config::load_global();
    let auth_storage = get_auth_store(&config).into_diagnostic()?;

    let opts = args.upload_opts.with_auth_store(Some(auth_storage));

    rattler_upload::upload_from_args(opts).await
}
