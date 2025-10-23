use clap::Parser;
use rattler_upload::upload::opt::UploadOpts;

/// Upload a conda package
///
/// With this command, you can upload a conda package to a channel.
/// Supported servers are Prefix.dev, Quetz, Anaconda.org, Artifactory, and S3 buckets.
///
/// ## Examples:
///
///   1. `pixi upload prefix --channel my_channel my_package.conda`
///
///   2. `pixi upload quetz --url <https://quetz.example.com> --channel my_channel my_package.conda`
///
///   3. `pixi upload anaconda --owner my_user my_package.conda`
///
///   4. `pixi upload s3 --bucket my-bucket --prefix my-prefix my_package.conda`
///
/// Use `pixi auth login` to authenticate with the server.
#[derive(Parser, Debug)]
#[command(name = "upload")]
pub struct Args {
    #[command(flatten)]
    pub upload_opts: UploadOpts,
}

/// Upload a package to a channel
pub async fn execute(args: Args) -> miette::Result<()> {
    // Execute the upload using rattler_upload
    rattler_upload::upload_from_args(args.upload_opts).await
}
