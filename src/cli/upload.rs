use std::path::PathBuf;

use clap::Parser;
use rattler_networking::AuthenticatedClient;

use rattler_digest::{compute_file_digest, Sha256};

/// Upload a package to a prefix.dev channel
#[derive(Parser, Debug)]
pub struct Args {
    /// The host + channel to upload to
    host: String,

    /// The file to upload
    package_file: PathBuf,
}


/// Upload a package to a prefix.dev channel
pub async fn execute(args: Args) -> anyhow::Result<()> {
    println!("Uploading package to {}...", args.host);
    println!("Package file: {}", args.package_file.display());

    let client = AuthenticatedClient::default();

    let sha256sum = format!("{:x}", compute_file_digest::<Sha256>(&args.package_file)?);
    let filename = args.package_file.file_name().unwrap().to_string_lossy().to_string();
    let filesize = args.package_file.metadata()?.len();

    let response = client
        .post(args.host.clone())
        .header("X-File-Sha256", sha256sum)
        .header("X-File-Name", filename)
        .header("Content-Length", filesize)
        .header("Content-Type", "application/octet-stream")
        .body(std::fs::read(&args.package_file)?)
        .send()
        .await?
    ;

    if response.status().is_success() {
        println!("Upload successful!");
    } else {
        println!("Upload failed!");

        if response.status() == 401 {
            println!("Authentication failed! Did you run `pixi auth login {}`?", args.host);
        }

        std::process::exit(1);
    }

    Ok(())
}