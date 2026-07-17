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
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    /// Run without network access. Uploading always requires the network, so
    /// this makes `pixi upload` fail fast instead of attempting to connect.
    /// Defined here rather than through the shared config flags because
    /// `UploadOpts` already owns `--auth-file`.
    #[arg(
        long,
        env = "PIXI_OFFLINE",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "true",
        value_parser = clap::builder::BoolishValueParser::new(),
    )]
    pub offline: Option<bool>,

    #[command(flatten)]
    pub upload_opts: UploadOpts,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Get authentication storage using pixi's auth system which respects
    // the authentication_override_file configuration
    let config = Config::load_global_with(&args.config_source.source());

    // Uploading a package always requires network access, so bail out early
    // with a clear error in offline mode. The `--offline` flag (with its
    // `PIXI_OFFLINE` env fallback) overrides the config-file value.
    if args.offline.unwrap_or_else(|| config.offline()) {
        return Err(crate::offline::NetworkRequiredError {
            command: "pixi upload",
        }
        .into());
    }

    let auth_storage = get_auth_store(&config).into_diagnostic()?;

    let opts = args.upload_opts.with_auth_store(Some(auth_storage));

    rattler_upload::upload_from_args(opts).await
}
