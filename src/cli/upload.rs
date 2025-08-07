use pixi_config::Config;
use pixi_utils::reqwest::auth_store;
use rattler_upload::upload::opt::UploadOpts;
use rattler_upload::upload_from_args;

/// Upload a package to a prefix.dev channel
pub async fn execute(mut args: UploadOpts) -> miette::Result<()> {
    let config = Config::load_global();
    match auth_store(&config) {
        Ok(store) => {
            args.auth_store = Some(store);
            upload_from_args(args).await
        }
        Err(_) => {
            args.auth_store = None;
            upload_from_args(args).await
        }
    }
}
