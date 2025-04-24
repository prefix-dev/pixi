// TODO: replace this with rattler-build upload after it moved into the rattler crate

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use futures::TryStreamExt;
use indicatif::HumanBytes;
use miette::{Context, Diagnostic, IntoDiagnostic};
use reqwest::StatusCode;

use rattler_digest::{Sha256, compute_file_digest};
use rattler_networking::AuthenticationMiddleware;
use thiserror::Error;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

use pixi_progress;
use pixi_utils::reqwest::reqwest_client_builder;

#[allow(rustdoc::bare_urls)]
/// Upload a conda package
///
/// With this command, you can upload a conda package to a channel.
/// Example: `pixi upload https://prefix.dev/api/v1/upload/my_channel my_package.conda`
///
/// Use `pixi auth login` to authenticate with the server.
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
        .wrap_err_with(|| {
            miette::miette!("{} should have a file name", args.package_file.display())
        })?
        .to_string_lossy()
        .to_string();

    let filesize = args.package_file.metadata().into_diagnostic()?.len();

    println!("Uploading package to: {}", args.host);
    println!(
        "Package file:         {} ({})\n",
        args.package_file.display(),
        HumanBytes(filesize)
    );

    let client = reqwest_middleware::ClientBuilder::new(
        reqwest_client_builder(None)?.build().into_diagnostic()?,
    )
    .with_arc(Arc::new(
        AuthenticationMiddleware::from_env_and_defaults().into_diagnostic()?,
    ))
    .build();

    let sha256sum = format!(
        "{:x}",
        compute_file_digest::<Sha256>(&args.package_file).into_diagnostic()?
    );

    let file = File::open(&args.package_file).await.into_diagnostic()?;

    let progress_bar = indicatif::ProgressBar::new(filesize)
        .with_prefix("Uploading")
        .with_style(pixi_progress::default_bytes_style());

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
        .map_err(|e| UploadError::RequestFailed {
            host: args.host.clone(),
            source: e,
        })?;

    match response.status() {
        StatusCode::OK => {
            eprintln!(
                "{} Package uploaded successfully!",
                console::style("✔").green()
            );
        }
        StatusCode::UNAUTHORIZED => {
            return Err(UploadError::Unauthorized {
                host: args.host.clone(),
                source: response
                    .error_for_status()
                    .expect_err("capture reqwest error"),
            }
            .into());
        }
        StatusCode::INTERNAL_SERVER_ERROR => {
            return Err(UploadError::ServerError {
                host: args.host.clone(),
                source: response
                    .error_for_status()
                    .expect_err("capture reqwest error"),
            }
            .into());
        }
        StatusCode::CONFLICT => {
            return Err(UploadError::Conflict {
                host: args.host.clone(),
                source: response
                    .error_for_status()
                    .expect_err("capture reqwest error"),
            }
            .into());
        }
        status => {
            return Err(UploadError::UnexpectedStatus {
                host: args.host.clone(),
                status,
                source: response
                    .error_for_status()
                    .expect_err("capture reqwest error"),
            }
            .into());
        }
    }

    Ok(())
}

#[derive(Debug, Error, Diagnostic)]
pub enum UploadError {
    #[error("Failed to send request to {host}")]
    #[diagnostic(help("Check if the host is correct and reachable."))]
    RequestFailed {
        host: String,
        #[source]
        source: reqwest_middleware::Error,
    },

    #[error("Unauthorized request to {host}")]
    #[diagnostic(help("Try logging in with `pixi auth login`."))]
    Unauthorized {
        host: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("Server error at {host}")]
    #[diagnostic(help("The server encountered an internal error. Try again later."))]
    ServerError {
        host: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("Unexpected response from {host}: {status}")]
    #[diagnostic(help("Unexpected status code, verify the API specification."))]
    UnexpectedStatus {
        host: String,
        status: StatusCode,
        #[source]
        source: reqwest::Error,
    },

    #[error("Conflict: The package likely already exists in the channel: {host}")]
    #[diagnostic(help("Try changing the package version or build number."))]
    Conflict {
        host: String,
        #[source]
        source: reqwest::Error,
    },
}
