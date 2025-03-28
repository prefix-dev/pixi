use std::io::{Seek, Write};

use flate2::read::GzDecoder;
use tar::Archive;

use miette::IntoDiagnostic;
use pixi_consts::consts;
use pixi_utils::reqwest::{build_reqwest_clients, reqwest_client_builder};
use reqwest::redirect::Policy;

use tempfile::{NamedTempFile, TempDir};
use url::Url;

use rattler_conda_types::Version;
use std::str::FromStr;

/// Update pixi to the latest version or a specific version.
#[derive(Debug, clap::Parser)]
pub struct Args {
    /// The desired version (to downgrade or upgrade to).
    #[clap(long, conflicts_with = "force_latest")]
    version: Option<Version>,

    /// Force upgrade to the latest version, ignore with the current version.
    #[clap(long, default_value_t = false, conflicts_with = "version")]
    force_latest: bool,
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

async fn latest_version() -> miette::Result<Version> {
    // Uses the public Github Releases /latest endpoint to get the latest tag from the URL
    let url = format!("{}/latest", consts::RELEASES_URL);

    // Create a client with a redirect policy
    let no_redirect_client = reqwest_client_builder(None)?
        .redirect(Policy::none())
        .build()
        .into_diagnostic()?; // Prevent automatic redirects

    let version: String = match no_redirect_client
        .head(&url)
        .header("User-Agent", user_agent())
        .send()
        .await
    {
        Ok(response) => {
            if response.status().is_redirection() {
                match response.headers().get("Location") {
                    Some(location) => Url::parse(location.to_str().into_diagnostic()?)
                        .into_diagnostic()?
                        .path_segments()
                        .ok_or_else(|| {
                            miette::miette!("Could not get segments from Location header")
                        })?
                        .last()
                        .ok_or_else(|| {
                            miette::miette!("Could not get version from Location header")
                        })?
                        .to_string(),
                    None => miette::bail!(
                        "URL: {}. Redirect detected, but no 'Location' header found.",
                        url
                    ),
                }
            } else {
                miette::bail!(
                    "URL: {}. Request failed or did not redirect: {}.",
                    url,
                    response.status()
                )
            }
        }
        Err(err) => miette::bail!("URL: {}. Request failed: {}", url, err),
    };
    if version == "releases" {
        // /latest redirect took us back to /releases instead of /vX.Y.Z
        miette::bail!("URL '{}' does not seem to contain any releases.", url)
    } else if !version.starts_with("v") {
        miette::bail!("Tag name '{}' must start with v.", version)
    } else {
        Ok(Version::from_str(&version[1..]).into_diagnostic()?)
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut is_resolved = false;
    let target_version = if args.force_latest {
        None
    } else {
        // Get the target version, without 'v' prefix
        match args.version {
            Some(version) => Some(version),
            None => {
                is_resolved = true;
                Some(latest_version().await?)
            }
        }
    };

    let target_version = target_version.as_ref();

    // Get the current version of the pixi binary
    let current_version = Version::from_str(consts::PIXI_VERSION).into_diagnostic()?;

    // Stop here if the target version is the same as the current version
    if target_version.is_some_and(|t| *t == current_version) {
        eprintln!(
            "{}pixi is already up-to-date (version {})",
            console::style(console::Emoji("✔ ", "")).green(),
            current_version
        );
        return Ok(());
    }

    let action = match target_version {
        Some(target_version) if *target_version < current_version && is_resolved => {
            // Ask if --version was not passed
            let confirmation = dialoguer::Confirm::new()
                .with_prompt(format!(
                    "\nCurrent version ({}) is more recent than remote ({}). Do you want to downgrade?",
                    current_version, target_version
                ))
                .default(false)
                .show_default(true)
                .interact()
                .into_diagnostic()?;
            if !confirmation {
                return Ok(());
            };
            "downgraded"
        }
        Some(target_version) if *target_version < current_version && !is_resolved => "downgraded",
        _ => "upgrade",
    };

    if let Some(target_version) = target_version {
        eprintln!(
            "{}Pixi will be {} from {} to {}",
            console::style(console::Emoji("✔ ", "")).green(),
            action,
            current_version,
            target_version
        );
    } else {
        eprintln!(
            "{}Pixi will be force {} to latest",
            console::style(console::Emoji("✔ ", "")).green(),
            action
        );
    }

    // Get the name of the binary to download and install based on the current platform
    let archive_name = default_archive_name()
        .expect("Could not find the default archive name for the current platform");

    let download_url = if let Some(target_version) = target_version {
        format!(
            "{}/download/v{}/{}",
            consts::RELEASES_URL,
            target_version,
            archive_name
        )
    } else {
        format!("{}/latest/download/{}", consts::RELEASES_URL, archive_name)
    };
    // Create a temp file to download the archive
    let mut archived_tempfile = tempfile::NamedTempFile::new().into_diagnostic()?;

    let client = build_reqwest_clients(None, None)?.1;
    let mut res = client
        .get(&download_url)
        .header("User-Agent", user_agent())
        .send()
        .await
        .expect("Failed to download the archive");

    if res.status() != reqwest::StatusCode::OK {
        return Err(miette::miette!(format!(
            "URL {} returned {}",
            download_url,
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

    if let Some(target_version) = target_version {
        eprintln!(
            "{}Pixi has been updated to version {}.",
            console::style(console::Emoji("✔ ", "")).green(),
            target_version
        );
    } else {
        eprintln!(
            "{}Pixi has been updated to latest release.",
            console::style(console::Emoji("✔ ", "")).green(),
        );
    }

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
