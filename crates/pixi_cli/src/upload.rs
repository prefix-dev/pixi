// TODO: replace this with rattler-build upload after it moved into the rattler crate

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, ValueEnum};
use futures::TryStreamExt;
use indicatif::HumanBytes;
use miette::{Context, Diagnostic, IntoDiagnostic};
use reqwest::{Method, StatusCode};

use rattler_conda_types::package::IndexJson;
use rattler_digest::{Sha256, compute_file_digest};
use rattler_package_streaming::{seek, ExtractError};
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
#[derive(Copy, Clone, Debug, ValueEnum, Eq, PartialEq)]
pub enum UploadMethod {
    Post,
    Put,
}

impl From<UploadMethod> for Method {
    fn from(value: UploadMethod) -> Self {
        match value {
            UploadMethod::Post => Method::POST,
            UploadMethod::Put => Method::PUT,
        }
    }
}

#[derive(Parser, Debug)]
pub struct Args {
    /// HTTP method to use when uploading (POST for prefix.dev, PUT for Artifactory)
    #[arg(long, value_enum, default_value = "post")]
    method: UploadMethod,

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

    let target_url = match args.method {
        UploadMethod::Post => args.host.clone(),
        UploadMethod::Put => {
            if looks_like_file_target(&args.host) {
                args.host.clone()
            } else {
                build_put_target(&args.host, filename.as_str(), &args.package_file)
                    .map_err(miette::ErrReport::from)?
            }
        }
    };

    if matches!(args.method, UploadMethod::Put) && target_url != args.host {
        println!("Resolved upload URL: {}", target_url);
    }

    let mut request = client
        .request(args.method.into(), target_url.clone())
        .header("X-File-Sha256", sha256sum)
        .header("Content-Length", filesize)
        .header("Content-Type", "application/octet-stream");

    if matches!(args.method, UploadMethod::Post) {
        request = request.header("X-File-Name", filename);
    }

    let response = request
        .body(body)
        .send()
        .await
        .map_err(|e| UploadError::RequestFailed {
            host: target_url.clone(),
            source: e,
        })?;

    match response.status() {
        StatusCode::OK | StatusCode::CREATED | StatusCode::NO_CONTENT => {
            eprintln!(
                "{} Package uploaded successfully!",
                console::style("✔").green()
            );
        }
        StatusCode::UNAUTHORIZED => {
            return Err(UploadError::Unauthorized {
                host: target_url.clone(),
                source: response
                    .error_for_status()
                    .expect_err("capture reqwest error"),
            }
            .into());
        }
        StatusCode::INTERNAL_SERVER_ERROR => {
            return Err(UploadError::ServerError {
                host: target_url.clone(),
                source: response
                    .error_for_status()
                    .expect_err("capture reqwest error"),
            }
            .into());
        }
        StatusCode::CONFLICT => {
            return Err(UploadError::Conflict {
                host: target_url.clone(),
                source: response
                    .error_for_status()
                    .expect_err("capture reqwest error"),
            }
            .into());
        }
        status => {
            return Err(UploadError::UnexpectedStatus {
                host: target_url,
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

fn looks_like_file_target(target: &str) -> bool {
    target.ends_with(".conda")
        || target.ends_with(".tar.bz2")
        || target.split('/').last().map_or(false, |segment| segment.contains('.'))
}

fn build_put_target(host: &str, filename: &str, package_path: &Path) -> Result<String, UploadError> {
    let index_json: IndexJson =
        seek::read_package_file(package_path).map_err(|source| {
            UploadError::ReadIndexJson {
                path: package_path.to_path_buf(),
                source,
            }
        })?;

    let subdir = index_json
        .subdir
        .as_deref()
        .filter(|value| !value.is_empty());

    let mut target = host.trim_end_matches('/').to_string();
    if let Some(subdir) = subdir {
        target.push('/');
        target.push_str(subdir);
    }
    target.push('/');
    target.push_str(filename);
    Ok(target)
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

    #[error("Failed to read index metadata from {path}")]
    #[diagnostic(help("Ensure the file is a valid conda package."))]
    ReadIndexJson {
        path: PathBuf,
        #[source]
        source: ExtractError,
    },
}
