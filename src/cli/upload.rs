use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use futures::TryStreamExt;
use indicatif::HumanBytes;
use miette::IntoDiagnostic;

use rattler_digest::{compute_file_digest, Sha256};
use rattler_networking::AuthenticationMiddleware;

use tokio::fs::File;
use tokio_util::io::ReaderStream;

use crate::progress;

/// Upload a package to a prefix.dev channel
#[derive(Parser, Debug)]
pub struct Args {
    /// The host + channel to upload to
    host: String,

    /// The file to upload
    package_file: PathBuf,
}

/// Upload a package to a prefix.dev channel
pub async fn execute(args: Args) -> miette::Result<()> {
    let filename = args
        .package_file
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let filesize = args.package_file.metadata().into_diagnostic()?.len();

    println!("Uploading package to: {}", args.host);
    println!(
        "Package file:         {} ({})\n",
        args.package_file.display(),
        HumanBytes(filesize)
    );

    let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
        .with_arc(Arc::new(AuthenticationMiddleware::default()))
        .build();

    let sha256sum = format!(
        "{:x}",
        compute_file_digest::<Sha256>(&args.package_file).into_diagnostic()?
    );

    let file = File::open(&args.package_file).await.into_diagnostic()?;

    let progress_bar = indicatif::ProgressBar::new(filesize)
        .with_prefix("Uploading")
        .with_style(progress::default_bytes_style());

    let reader_stream = ReaderStream::new(file)
        .inspect_ok(move |bytes| {
            progress_bar.inc(bytes.len() as u64);
        })
        .inspect_err(|e| {
            println!("Error while uploading: {}", e);
        });

    let body = reqwest::Body::wrap_stream(reader_stream);

    let response = client
        .post(args.host.clone())
        .header("X-File-Sha256", sha256sum)
        .header("X-File-Name", filename)
        .header("Content-Length", filesize)
        .header("Content-Type", "application/octet-stream")
        .body(body)
        .send()
        .await
        .into_diagnostic()?;

    if response.status().is_success() {
        println!("Upload successful!");
    } else {
        println!("Upload failed!");

        if response.status() == 401 {
            println!(
                "Authentication failed! Did you run `pixi auth login {}`?",
                args.host
            );
        }

        std::process::exit(1);
    }

    Ok(())
}
