use std::io::{Seek, Write};

use flate2::read::GzDecoder;
use tar::Archive;

use miette::IntoDiagnostic;
use pixi_consts::consts;
use reqwest::redirect::Policy;
use reqwest::Client;

use tempfile::{NamedTempFile, TempDir};
use url::Url;

/// Update pixi to the latest version or a specific version.
#[derive(Debug, clap::Parser)]
pub struct Args {
    /// The desired version (to downgrade or upgrade to). Update to the latest version if not specified.
    #[clap(long)]
    version: Option<String>,

    /// The URL where to find Pixi releases. Must provide a Github Releases-like HTTP API.
    /// - Downloads must be available at ${base-url}/download/v{X.Y.Z}/pixi-unknown-linux-musl
    /// - ${base-url}/latest must either redirect to ${base-url}/v{X.Y.Z} or serve the bytes for v{X.Y.Z}
    #[clap(long, default_value = "https://github.com/prefix-dev/pixi/releases/")]
    base_url: Option<Url>,
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
    } else if cfg!(target_os = "windows") {
        if cfg!(target_arch = "x86_64") {
            Some("pixi-x86_64-pc-windows-msvc.zip".to_string())
        } else if cfg!(target_arch = "aarch64") {
            Some("pixi-aarch64-pc-windows-msvc.zip".to_string())
        } else {
            None
        }
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

async fn latest_version(base_url: Url) -> miette::Result<String> {
    // Uses the public Github Releases /latest endpoint to get the latest tag from the URL
    // If base_url doesn't offer redirects (e.g. static site), then /latest
    // must be a file whose contents are the latest tag name.
    let url = base_url.join("latest").into_diagnostic()?;
    // Create a client with a redirect policy
    let no_redirect_client = Client::builder()
        .redirect(Policy::none()) // Prevent automatic redirects
        .build()
        .into_diagnostic()?;

    match no_redirect_client
        .head(url.clone())
        .header("User-Agent", user_agent())
        .send()
        .await
    {
        Ok(response) => {
            if response.status().is_redirection() {
                match response.headers().get("Location") {
                    Some(location) => Ok(Url::parse(&location.to_str().into_diagnostic()?)
                        .into_diagnostic()?
                        .path_segments()
                        .ok_or_else(|| {
                            miette::miette!("Could not get segments from Location header")
                        })?
                        .last()
                        .ok_or_else(|| {
                            miette::miette!("Could not get version from Location header")
                        })?
                        .to_string()),
                    None => miette::bail!(
                        "URL: {}. Redirect detected, but no 'Location' header found.",
                        url
                    ),
                }
            } else if response.status().is_success() {
                // No redirection, but response is ok, check contents
                let client = Client::new();
                match client
                    .get(url.clone())
                    .header("User-Agent", user_agent())
                    .send()
                    .await
                {
                    Ok(res) => Ok(res.text().await.into_diagnostic()?.trim().to_string()),
                    Err(err) => miette::bail!("URL: {}. Request failed: {}", url, err),
                }
            } else {
                miette::bail!("URL: {}. Request failed: {}.", url, response.status())
            }
        }
        Err(err) => miette::bail!("URL: {}. Request failed: {}", url, err),
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut base_url = args
        .base_url
        .ok_or_else(|| miette::miette!("Bad value for --base-url"))?;
    if !base_url.path().ends_with("/") {
        base_url.set_path(format!("{}/", base_url.path()).as_str())
    }

    // Get the target version
    let target_version = match args.version {
        Some(version) => version,
        None => latest_version(base_url.clone()).await?,
    };

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

    let url = base_url
        .join(&format!("download/{}/{}", target_version, archive_name).as_str())
        .into_diagnostic()?;
    // Create a temp file to download the archive
    let mut archived_tempfile = tempfile::NamedTempFile::new().into_diagnostic()?;

    let client = Client::new();
    let mut res = client
        .get(url.clone())
        .header("User-Agent", user_agent())
        .send()
        .await
        .expect("Failed to download the archive");

    if res.status() != reqwest::StatusCode::OK {
        return Err(miette::miette!(format!(
            "URL {} returned {}",
            url,
            res.status()
        )));
    } else {
        // Download the archive
        while let Some(chunk) = res.chunk().await.into_diagnostic()? {
            archived_tempfile
                .as_file()
                .write_all(&chunk)
                .into_diagnostic()?;
        }
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
        unpack_tar_gz(&archived_tempfile, binary_tempdir)?;
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

/// Unpack files from a tar.gz archive to a target directory.
fn unpack_tar_gz(
    archived_tempfile: &NamedTempFile,
    binary_tempdir: &TempDir,
) -> miette::Result<()> {
    let mut archive = Archive::new(GzDecoder::new(archived_tempfile.as_file()));

    for entry in archive.entries().into_diagnostic()? {
        let mut entry = entry.into_diagnostic()?;
        let path = entry.path().into_diagnostic()?;

        // Skip directories; we only care about files.
        if entry.header().entry_type().is_file() {
            // Create a flat path by stripping any directory components.
            let stripped_path = path
                .file_name()
                .ok_or_else(|| miette::miette!("Failed to extract file name from {:?}", path))?;

            // Construct the final path in the target directory.
            let final_path = binary_tempdir.path().join(stripped_path);

            // Unpack the file to the destination.
            entry.unpack(final_path).into_diagnostic()?;
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    pub fn test_unarchive_flat_structure() {
        // This archive contains a single file named "a_file"
        // So we expect the file to be extracted to the target directory

        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let archive_path = manifest_dir
            .join("tests")
            .join("data")
            .join("archives")
            .join("pixi_flat_archive.tar.gz");

        let named_tempfile = tempfile::NamedTempFile::new().unwrap();
        let binary_tempdir = tempfile::tempdir().unwrap();

        fs_err::copy(archive_path, named_tempfile.path()).unwrap();

        super::unpack_tar_gz(&named_tempfile, &binary_tempdir).unwrap();

        let binary_path = binary_tempdir.path().join("a_file");
        assert!(binary_path.exists());
    }

    #[test]
    pub fn test_unarchive_nested_structure() {
        // This archive contains following nested structure
        // pixi_nested_archive.tar.gz
        // ├── some_pixi (directory)
        // │   └── some_pixi (file)
        // So we want to test that we can extract only the file to the target directory
        // without parent directory
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let archive_path = manifest_dir
            .join("tests")
            .join("data")
            .join("archives")
            .join("pixi_nested_archive.tar.gz");

        let named_tempfile = tempfile::NamedTempFile::new().unwrap();
        let binary_tempdir = tempfile::tempdir().unwrap();

        fs_err::copy(archive_path, named_tempfile.path()).unwrap();

        super::unpack_tar_gz(&named_tempfile, &binary_tempdir).unwrap();

        let binary_path = binary_tempdir.path().join("some_pixi");
        assert!(binary_path.exists());
        assert!(binary_path.is_file());
    }
}
