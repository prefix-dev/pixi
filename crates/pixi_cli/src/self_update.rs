use std::cmp::Ordering;
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

use crate::GlobalOptions;
use pixi_reporters::format_release_notes;

/// Update pixi to the latest version or a specific version.
#[derive(Debug, clap::Parser)]
pub struct Args {
    /// The desired version (to downgrade or upgrade to).
    #[clap(long)]
    version: Option<Version>,

    /// Only show release notes, do not modify the binary.
    #[clap(long)]
    dry_run: bool,

    /// Force download the desired version when not exactly same with the current. If no desired
    /// version, always replace with the latest version.
    #[clap(long, default_value_t = false)]
    force: bool,

    /// Skip printing the release notes.
    #[clap(long, default_value_t = false)]
    no_release_note: bool,
}

/// Response from the Github API when fetching a release by tag.
/// https://docs.github.com/de/rest/releases/releases?apiVersion=2022-11-28#get-a-release-by-tag-name
#[derive(Debug, serde::Deserialize)]
struct ReleaseResponse {
    /// Markdown body of the release as seen on the Github release page.
    body: String,

    /// The time and date when the release was published. (seems to be ISO 8601)
    published_at: String,

    /// The tag name of the release
    tag_name: String,
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

async fn fetch_release_notes(version: &Option<Version>) -> miette::Result<String> {
    let url = if let Some(version) = version {
        format!("{}/v{}", consts::RELEASES_API_BY_TAG, version)
    } else {
        consts::RELEASES_API_LATEST.to_string()
    };

    let client = build_reqwest_clients(None, None)?.1;
    let response = client
        .get(&url)
        .header("User-Agent", user_agent())
        .send()
        .await
        .into_diagnostic()?;

    if response.status().is_success() {
        let release_response: ReleaseResponse = response.json().await.into_diagnostic()?;

        // We only care for the date, not the time
        let date = release_response
            .published_at
            .split('T')
            .next()
            .unwrap_or("unknown");

        Ok(format!(
            "Release notes for version {} ({}):\n{}\n",
            version
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or(release_response.tag_name),
            date,
            release_response.body.trim()
        ))
    } else {
        miette::bail!("Status code {}", response.status())
    }
}

/// Executes the self-update command.
///
/// # Arguments
/// * `args` - The self-update specific arguments.
/// * `global_options` - Reference to the global CLI options.
pub async fn execute(args: Args, global_options: &GlobalOptions) -> miette::Result<()> {
    let is_quiet = global_options.quiet > 0;
    // Get the target version, without 'v' prefix, None for force latest version
    let target_version = match &args.version {
        Some(version) => {
            // Remove leading 'v' if present and inform the user
            if version.to_string().starts_with('v') {
                if !is_quiet {
                    eprintln!(
                        "{}Warning: Leading 'v' removed from version {}",
                        console::style(console::Emoji("⚠️ ", "")).yellow(),
                        version
                    );
                }
                Some(Version::from_str(&version.to_string()[1..]).into_diagnostic()?)
            } else {
                Some(version.clone())
            }
        }
        None => {
            if args.force {
                None
            } else {
                Some(latest_version().await?)
            }
        }
    };

    // Get the current version of the pixi binary
    let current_version = Version::from_str(consts::PIXI_VERSION).into_diagnostic()?;

    let up_to_date = target_version
        .as_ref()
        .is_some_and(|t| *t == current_version);

    let fetch_release_warning = if args.no_release_note || up_to_date || is_quiet {
        None
    } else {
        match fetch_release_notes(&target_version).await {
            Ok(release_notes) => {
                // Print release notes
                eprintln!(
                    "{}{}",
                    console::style(console::Emoji("📝 ", "")).yellow(),
                    format_release_notes(&release_notes)
                );
                None
            }
            Err(err) => {
                // Failure to fetch release notes must not prevent self-update, especially if format changes
                let release_url = if let Some(ref target_version) = target_version {
                    format!("{}/v{}", consts::RELEASES_URL, target_version)
                } else {
                    format!("{}/latest", consts::RELEASES_URL)
                };
                Some(format!(
                    "{}Failed to fetch release notes ({}). Check the release page for more information: {}",
                    console::style(console::Emoji("⚠️ ", "")).yellow(),
                    err,
                    release_url
                ))
            }
        }
    };

    // Don't actually update the binary if `--dry-run` is passed
    if args.dry_run {
        if !is_quiet {
            let target_version = match target_version {
                Some(target_version) => target_version,
                None => latest_version().await?,
            };
            eprintln!("{}", get_dry_run_message(&current_version, &target_version));
        }
        return Ok(());
    }

    // Stop here if the target version is the same as the current version
    if up_to_date {
        if !is_quiet {
            eprintln!(
                "{}pixi is already up-to-date (version {})",
                console::style(console::Emoji("✔ ", "")).green(),
                current_version
            );
        }
        return Ok(());
    }

    let action = if !args.force
        && target_version
            .as_ref()
            .is_some_and(|t| *t < current_version)
    {
        if args.version.is_none() {
            // Ask if --version was not passed
            let confirmation = dialoguer::Confirm::new()
                .with_prompt(format!(
                        "\nCurrent version ({}) is more recent than remote ({}). Do you want to downgrade?",
                        current_version, target_version.as_ref().expect("target_version is not resolved")
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

    if !args.force && !is_quiet {
        eprintln!(
            "{}Pixi will be {} from {} to {}",
            console::style(console::Emoji("✔ ", "")).green(),
            action,
            current_version,
            target_version
                .as_ref()
                .expect("target_version is not resolved")
        );
    }

    // Get the name of the binary to download and install based on the current platform
    let archive_name = default_archive_name()
        .expect("Could not find the default archive name for the current platform");

    let download_url = if let Some(ref target_version) = target_version {
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
    let mut archived_tempfile = NamedTempFile::new().into_diagnostic()?;

    let client = build_reqwest_clients(None, None)?.1;
    let mut res = client
        .get(&download_url)
        .header("User-Agent", user_agent())
        .send()
        .await
        .expect("Failed to download the archive");

    if res.status() != reqwest::StatusCode::OK {
        miette::bail!(format!("URL {} returned {}", download_url, res.status()));
    } else {
        // Download the archive
        while let Some(chunk) = res.chunk().await.into_diagnostic()? {
            archived_tempfile
                .as_file()
                .write_all(&chunk)
                .into_diagnostic()?;
        }
    }

    if !is_quiet {
        eprintln!(
            "{}Pixi archive downloaded.",
            console::style(console::Emoji("✔ ", "")).green(),
        );
    }

    // Seek to the beginning of the file before uncompressing it
    archived_tempfile
        .rewind()
        .expect("Failed to rewind the archive file");

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

    if !is_quiet {
        eprintln!(
            "{}Pixi archive uncompressed.",
            console::style(console::Emoji("✔ ", "")).green(),
        );
    }

    // Get the new binary path used for self-replacement
    let new_binary_path = binary_tempdir.path().join(pixi_binary_name());

    // Replace the current binary with the new binary
    self_replace::self_replace(new_binary_path).into_diagnostic()?;

    if !is_quiet {
        if let Some(ref target_version) = target_version {
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
    }

    if let Some(fetch_release_warning) = fetch_release_warning {
        tracing::warn!(fetch_release_warning);
    }

    Ok(())
}

/// Return the message that should be shown to users when executing with `--dry-run`.
fn get_dry_run_message(current: &Version, target: &Version) -> String {
    match target.cmp(current) {
        Ordering::Equal => format!(
            "{}Current pixi version already at latest version: {current}.",
            console::style(console::Emoji("✔ ", "")).green()
        ),
        Ordering::Greater => format!(
            "{}Pixi version would be updated from {current} to {target}, but `--dry-run` given.",
            console::style(console::Emoji("ℹ️ ", "")).yellow()
        ),
        Ordering::Less => format!(
            "{}Pixi version would be downgraded from {current} to {target}, but `--dry-run` given.",
            console::style(console::Emoji("ℹ️ ", "")).yellow()
        ),
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

pub async fn execute_stub(_: Args, _: &GlobalOptions) -> miette::Result<()> {
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

        let archive_path = PathBuf::from(env!("CARGO_WORKSPACE_DIR"))
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
        let archive_path = PathBuf::from(env!("CARGO_WORKSPACE_DIR"))
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
