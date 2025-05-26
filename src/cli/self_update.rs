use std::io::{Seek, Write};

use flate2::read::GzDecoder;
use tar::Archive;

use miette::IntoDiagnostic;
use pixi_config::Config;
use pixi_consts::consts;
use reqwest::Client;
use reqwest::redirect::Policy;

use tempfile::{NamedTempFile, TempDir};
use url::Url;

use rattler_conda_types::Version;
use std::fmt::Display;
use std::str::FromStr;

/// Update pixi to the latest version or a specific version.
#[derive(Debug, clap::Parser)]
pub struct Args {
    /// The desired version (to downgrade or upgrade to).
    #[clap(long)]
    version: Option<Version>,
    /// Take a fixed version of pixi from the specified URL. The URL must point to a pixi binary.
    #[clap(long, conflicts_with = "version")]
    url: Option<Url>,
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
    let mut no_redirect_client_builder = Client::builder().redirect(Policy::none()); // Prevent automatic redirects
    for p in Config::load_global().get_proxies().into_diagnostic()? {
        no_redirect_client_builder = no_redirect_client_builder.proxy(p);
    }
    let no_redirect_client = no_redirect_client_builder.build().into_diagnostic()?;

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
                        .next_back()
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

/// Downloads the target of an URL into a temporary file.
async fn download<U>(url: U, dest: &mut impl Write) -> miette::Result<()>
where
    U: reqwest::IntoUrl + Display,
{
    let url_as_str = url.as_str().to_owned();

    // TODO proxy inject in https://github.com/prefix-dev/pixi/pull/3346
    let client = Client::new();
    let mut res = client
        .get(url)
        .header("User-Agent", user_agent())
        .send()
        .await
        .expect("Failed to download the archive");

    if res.status() != reqwest::StatusCode::OK {
        return Err(miette::miette!(format!(
            "URL {} returned {}",
            url_as_str,
            res.status()
        )));
    } else {
        // Download the archive
        while let Some(chunk) = res.chunk().await.into_diagnostic()? {
            dest.write_all(&chunk).into_diagnostic()?;
        }
    }

    Ok(())
}

/// Unpacks a pixi archive (typically downloaded from GitHub releases) into a temporary directory.
async fn unpack_release_archive(
    mut archived_tempfile: NamedTempFile,
    archive_name: &str,
) -> miette::Result<TempDir> {
    // Seek to the beginning of the file before uncompressing it
    let _ = archived_tempfile.rewind();

    // Create a temporary directory to unpack the archive
    let binary_tempdir = tempfile::tempdir().into_diagnostic()?;

    // Uncompress the archive
    if archive_name.ends_with(".tar.gz") {
        unpack_tar_gz(&archived_tempfile, &binary_tempdir)?;
    } else if archive_name.ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(archived_tempfile.as_file()).into_diagnostic()?;
        archive.extract(&binary_tempdir).into_diagnostic()?;
    } else {
        let error_message = format!("Unsupported archive format: {}", archive_name);
        Err(miette::miette!(error_message))?
    }

    Ok(binary_tempdir)
}

pub async fn execute(args: Args) -> miette::Result<()> {
    if let Some(url) = args.url {
        let mut tempfile = tempfile::NamedTempFile::new().into_diagnostic()?;
        // If a URL is provided, download the archive from the URL
        download(url, &mut tempfile).await?;
        self_replace::self_replace(tempfile).into_diagnostic()?;

        eprintln!(
            "{}Pixi has been updated.",
            console::style(console::Emoji("✔ ", "")).green(),
        );

        Ok(())
    } else {
        // Get the target version, without 'v' prefix
        let target_version = match &args.version {
            Some(version) => version,
            None => &latest_version().await?,
        };

        // Get the current version of the pixi binary
        let current_version = Version::from_str(consts::PIXI_VERSION).into_diagnostic()?;

        // Stop here if the target version is the same as the current version
        if *target_version == current_version {
            eprintln!(
                "{}pixi is already up-to-date (version {})",
                console::style(console::Emoji("✔ ", "")).green(),
                current_version
            );
            return Ok(());
        }

        let action = if *target_version < current_version {
            if args.version.is_none() {
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
            };
            "downgraded"
        } else {
            "updated"
        };

        // Get the name of the binary to download and install based on the current platform
        let archive_name = default_archive_name()
            .expect("Could not find the default archive name for the current platform");

        eprintln!(
            "{}Pixi will be {} from {} to {}",
            console::style(console::Emoji("✔ ", "")).green(),
            action,
            current_version,
            target_version
        );

        let download_url = format!(
            "{}/download/v{}/{}",
            consts::RELEASES_URL,
            target_version,
            archive_name
        );

        let mut archive = tempfile::NamedTempFile::new().into_diagnostic()?;
        // Otherwise, download the latest pixi archive from the default releases repos
        download(download_url, &mut archive).await?;

        eprintln!(
            "{}Pixi archive downloaded.",
            console::style(console::Emoji("✔ ", "")).green(),
        );

        let binary_tempdir = unpack_release_archive(archive, &archive_name).await?;

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
