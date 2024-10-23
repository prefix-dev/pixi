use std::io::{Seek, Write};

use flate2::read::GzDecoder;
use tar::Archive;

use miette::{Context, IntoDiagnostic};
use pixi_consts::consts;
use reqwest::Client;
use serde::Deserialize;

/// Update pixi to the latest version or a specific version.
#[derive(Debug, clap::Parser)]
pub struct Args {
    /// The desired version (to downgrade or upgrade to). Update to the latest version if not specified.
    #[clap(long)]
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
}

fn user_agent() -> String {
    format!("pixi {}", consts::PIXI_VERSION)
}

fn default_archive_name() -> Option<String> {
    if cfg!(target_os = "macos") {
        if cfg!(target_arch = "x86_64") {
            Some("pixi-x86_64-apple-darwin.tar.gz".to_string())
        } else {
            Some("pixi-aarch64-apple-darwin.tar.gz".to_string())
        }
    } else if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
        Some("pixi-x86_64-pc-windows-msvc.zip".to_string())
    } else if cfg!(target_os = "linux") {
        if cfg!(target_arch = "x86_64") {
            Some("pixi-x86_64-unknown-linux-musl.tar.gz".to_string())
        } else if cfg!(target_arch = "aarch64") {
            Some("pixi-aarch64-unknown-linux-musl.tar.gz".to_string())
        } else {
            None
        }
    } else {
        None
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Retrieve the target version information from github.
    let target_version_json = match retrieve_target_version(&args.version).await {
        Ok(target_version_json) => target_version_json,
        Err(err) => match args.version {
            Some(version) => {
                miette::bail!("The version you specified is not available: {}", version)
            }
            None => miette::bail!("Failed to fetch latest version from github: {}", err),
        },
    };

    // Get the target version
    let target_version = target_version_json.tag_name.trim_start_matches('v');

    // Get the current version of the pixi binary
    let current_version = consts::PIXI_VERSION;

    // Stop here if the target version is the same as the current version
    if target_version == current_version {
        eprintln!(
            "{}pixi is already up-to-date (version {})",
            console::style(console::Emoji("✔ ", "")).green(),
            current_version
        );
        return Ok(());
    }

    eprintln!(
        "{}Pixi will be updated from {} to {}",
        console::style(console::Emoji("✔ ", "")).green(),
        current_version,
        target_version
    );

    // Get the name of the binary to download and install based on the current platform
    let archive_name = default_archive_name()
        .expect("Could not find the default archive name for the current platform");

    let url = target_version_json
        .assets
        .iter()
        .find(|asset| asset.name == archive_name)
        .expect("Could not find the archive in the release")
        .browser_download_url
        .clone();

    // Create a temp file to download the archive
    let mut archived_tempfile = tempfile::NamedTempFile::new().into_diagnostic()?;

    let client = Client::new();
    let mut res = client
        .get(&url)
        .header("User-Agent", user_agent())
        .send()
        .await
        .expect("Failed to download the archive");

    // Download the archive
    while let Some(chunk) = res.chunk().await.into_diagnostic()? {
        archived_tempfile
            .as_file()
            .write_all(&chunk)
            .into_diagnostic()?;
    }

    eprintln!(
        "{}Pixi archive downloaded.",
        console::style(console::Emoji("✔ ", "")).green(),
    );

    // Seek to the beginning of the file before uncompressing it
    let _ = archived_tempfile.rewind();

    // Create a temporary directory to unpack the archive
    let binary_tempdir = &tempfile::tempdir().into_diagnostic()?;

    // Uncompress the archive
    if archive_name.ends_with(".tar.gz") {
        let mut archive = Archive::new(GzDecoder::new(archived_tempfile.as_file()));
        archive.unpack(binary_tempdir).into_diagnostic()?;
    } else if archive_name.ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(archived_tempfile.as_file()).into_diagnostic()?;
        archive.extract(binary_tempdir).into_diagnostic()?;
    } else {
        let error_message = format!("Unsupported archive format: {}", archive_name);
        Err(miette::miette!(error_message))?
    }

    eprintln!(
        "{}Pixi archive uncompressed.",
        console::style(console::Emoji("✔ ", "")).green(),
    );

    // Get the new binary path used for self-replacement
    let new_binary_path = binary_tempdir.path().join(pixi_binary_name());

    // Replace the current binary with the new binary
    self_replace::self_replace(new_binary_path).into_diagnostic()?;

    eprintln!(
        "{}Pixi has been updated to version {}.",
        console::style(console::Emoji("✔ ", "")).green(),
        target_version
    );

    Ok(())
}

async fn retrieve_target_version(version: &Option<String>) -> miette::Result<GithubRelease> {
    // Fetch the target version from github.
    // The target version is:
    // - the latest version if no version is specified
    // - the specified version if a version is specified
    let url = if let Some(version) = version {
        format!(
            "https://api.github.com/repos/prefix-dev/pixi/releases/tags/v{}",
            version
        )
    } else {
        "https://api.github.com/repos/prefix-dev/pixi/releases/latest".to_string()
    };

    let client = Client::new();

    let res = client
        .get(url)
        .header("User-Agent", user_agent())
        .send()
        .await
        .expect("Failed to fetch from GitHub, client panic.");

    // Catch errors from the GitHub API
    if !res.status().is_success() {
        return Err(miette::miette!(
            "Failed to fetch the release from github, status {}, body: {}",
            res.status(),
            res.text()
                .await
                .expect("Failed to fetch GitHub release body, body text panic.")
        ));
    }

    let body = res
        .text()
        .await
        .expect("Failed to fetch GitHub release body, body text panic.");

    // compare target version with current version
    serde_json::from_str::<GithubRelease>(&body)
        .into_diagnostic()
        .with_context(|| format!("Failed to parse the Release from github: {:#?}", body))
}

fn pixi_binary_name() -> String {
    format!("pixi{}", std::env::consts::EXE_SUFFIX)
}

pub async fn execute_stub(_: Args) -> miette::Result<()> {
    let message = option_env!("PIXI_SELF_UPDATE_DISABLED_MESSAGE");
    miette::bail!(
        message.unwrap_or("This version of pixi was built without self-update support. Please use your package manager to update pixi.")
    )
}
