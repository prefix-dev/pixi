use std::io::Write;

use miette::IntoDiagnostic;
use reqwest::Client;
use serde::Deserialize;

/// Update pixi to the latest version
#[derive(Debug, clap::Parser)]
pub struct Args {
    /// The desired version (to downgrade or upgrade to)
    #[clap(long)]
    version: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    name: String,
    body: String,
    html_url: String,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
}

fn user_agent() -> String {
    format!("pixi {}", env!("CARGO_PKG_VERSION"))
}

fn default_name() -> Option<String> {
    if cfg!(target_os = "macos") {
        if cfg!(target_arch = "x86_64") {
            Some("pixi-x86_64-apple-darwin".to_string())
        } else {
            Some("pixi-aarch64-apple-darwin".to_string())
        }
    } else if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
        Some("pixi-x86_64-pc-windows-msvc.exe".to_string())
    } else if cfg!(target_os = "linux") {
        if cfg!(target_arch = "x86_64") {
            Some("pixi-x86_64-unknown-linux-musl".to_string())
        } else if cfg!(target_arch = "aarch64") {
            Some("pixi-aarch64-unknown-linux-musl".to_string())
        } else {
            None
        }
    } else {
        None
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // fetch latest version from github
    let url = if let Some(version) = args.version {
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
        .unwrap();
    let body = res.text().await.unwrap();

    // compare latest version with current version
    let json = serde_json::from_str::<GithubRelease>(&body).unwrap();

    let version = json.tag_name.trim_start_matches('v');

    // if latest version is newer, download and replace binary
    if version != env!("CARGO_PKG_VERSION") {
        println!("A newer version is available: {}", version);
    } else {
        println!("You are already using the latest version: {}", version);
    }

    let name = default_name().unwrap();

    let url = json
        .assets
        .iter()
        .find(|asset| asset.name == name)
        .unwrap()
        .browser_download_url
        .clone();

    // if latest version is newer, download and replace binary (using self_replace)
    let tempfile = tempfile::NamedTempFile::new().into_diagnostic()?;

    let mut res = client
        .get(&url)
        .header("User-Agent", user_agent())
        .send()
        .await
        .unwrap();

    while let Some(chunk) = res.chunk().await.into_diagnostic()? {
        tempfile.as_file().write_all(&chunk).into_diagnostic()?;
    }

    self_replace::self_replace(&tempfile).into_diagnostic()?;

    Ok(())
}
