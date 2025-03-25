use std::io::{Seek, Write};

use flate2::read::GzDecoder;
use tar::Archive;

use miette::IntoDiagnostic;
use pixi_consts::consts;
use reqwest::redirect::Policy;
use reqwest::Client;

use tempfile::{NamedTempFile, TempDir};
use url::Url;

use console::Style;
use rattler_conda_types::Version;
use std::str::FromStr;

/// Update pixi to the latest version or a specific version.
#[derive(Debug, clap::Parser)]
pub struct Args {
    /// The desired version (to downgrade or upgrade to).
    #[clap(long)]
    version: Option<Version>,

    /// Only show release notes, do not modify the binary.
    #[clap(long)]
    release_notes_only: bool,
}

#[derive(Debug, serde::Deserialize)]
/// Response from the Github API when fetching a release by tag.
// https://docs.github.com/de/rest/releases/releases?apiVersion=2022-11-28#get-a-release-by-tag-name
struct ReleaseResponse {
    /// Markdown body of the release as seen on the Github release page.
    body: String,

    /// The time and date when the release was published. (seems to be ISO 8601)
    published_at: String,
}

/// Simple helper for coloring and discard certain elements in the release notes.
#[derive(Debug, Default)]
struct ReleaseNotesFormatter {
    /// Current state of the string builder.
    string_builder: String,

    /// Whether the current section should be discarded.
    discard_section: bool,
}

impl ReleaseNotesFormatter {
    /// Bloaty sections that we want to skip.
    const SKIPPED_SECTIONS: [&'static str; 2] = ["New Contributors", "Download pixi"];

    /// Create a new formatter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new formatter and feed the given markdown string into it.
    pub fn new_from_string(markdown: &str) -> Self {
        let mut formatter = Self::new();
        for line in markdown.lines() {
            formatter.append_line(line);
        }
        formatter
    }

    /// Check if the line starts a new markdown section.
    fn extract_section_name(line: &str) -> Option<&str> {
        let header_pattern =
            regex::Regex::new(r"^ {0,3}#+\s+(.+)$").expect("Invalid regex pattern");
        header_pattern
            .captures(line)
            .and_then(|captures| captures.get(1).map(|m| m.as_str()))
    }

    fn color_line(line: &str, string_builder: &mut String) {
        let base_style = match line.trim().chars().next() {
            Some('#') => Style::new().cyan(),
            Some('*') | Some('-') => Style::new().yellow(),
            _ => Style::new(),
        };

        string_builder.push_str(&base_style.apply_to(line).to_string());
    }

    /// Append the next line from the release notes.
    pub fn append_line(&mut self, line: &str) {
        let section_name = Self::extract_section_name(line);

        if let Some(section_name) = section_name {
            // Check for prefix, since download section is followed by version number
            self.discard_section = Self::SKIPPED_SECTIONS
                .iter()
                .any(|&s| section_name.starts_with(s));
        }

        if !self.discard_section {
            // Skip empty lines if the previous line was also empty (allowing only one empty line)
            if line.trim().is_empty() && self.string_builder.ends_with("\n\n") {
                return;
            }

            Self::color_line(line, &mut self.string_builder);
            self.string_builder.push('\n');
        }
    }

    /// Consumes the formatter and returns the formatted release notes.
    pub fn get_formatted_notes(self) -> String {
        self.string_builder
    }
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
    let no_redirect_client = Client::builder()
        .redirect(Policy::none()) // Prevent automatic redirects
        .build()
        .into_diagnostic()?;

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

async fn get_release_notes(version: &Version) -> miette::Result<String> {
    let url = format!("{}/v{}", consts::RELEASES_API_BY_TAG, version);

    let client = Client::new();
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
            version,
            date,
            release_response.body.trim()
        ))
    } else {
        miette::bail!("Status code {}", response.status())
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    // Get the target version, without 'v' prefix
    let target_version = match &args.version {
        Some(version) => {
            // Remove leading 'v' if present and inform the user
            if version.to_string().starts_with('v') {
                eprintln!(
                    "{}Warning: Leading 'v' removed from version {}",
                    console::style(console::Emoji("⚠️ ", "")).yellow(),
                    version
                );
                Version::from_str(&version.to_string()[1..]).into_diagnostic()?
            } else {
                version.clone()
            }
        }
        None => latest_version().await?,
    };
    // Get the current version of the pixi binary
    let current_version = Version::from_str(consts::PIXI_VERSION).into_diagnostic()?;

    // Get release notes
    // failure to fetch release notes must not prevent self-update, especially if format changes
    let release_notes = match get_release_notes(&target_version).await {
        Ok(release_notes) => format!(
            "{}{}",
            console::style(console::Emoji("📝 ", "")).yellow(),
            ReleaseNotesFormatter::new_from_string(&release_notes).get_formatted_notes()
        ),
        Err(err) => {
            let release_url = format!("{}/v{}", consts::RELEASES_URL, target_version);
            format!("{}Failed to fetch release notes ({}). Check the release page for more information: {}",
                console::style(console::Emoji("⚠️ ", "")).yellow(),
                err,
                release_url)
        }
    };

    // Print release notes
    eprintln!("{}", release_notes);

    // If the user only wants to see the release notes, print them and exit
    if args.release_notes_only {
        eprintln!(
            "{}To update to this version, run `pixi self-update --version {}`",
            console::style(console::Emoji("ℹ️ ", "")).yellow(),
            target_version
        );
        return Ok(());
    }

    // Stop here if the target version is the same as the current version
    if target_version == current_version {
        eprintln!(
            "{}pixi is already up-to-date (version {})",
            console::style(console::Emoji("✔ ", "")).green(),
            current_version
        );
        return Ok(());
    }

    let action = if target_version < current_version {
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

    eprintln!(
        "{}Pixi will be {} from {} to {}",
        console::style(console::Emoji("✔ ", "")).green(),
        action,
        current_version,
        target_version
    );

    // Get the name of the binary to download and install based on the current platform
    let archive_name = default_archive_name()
        .expect("Could not find the default archive name for the current platform");

    let download_url = format!(
        "{}/download/v{}/{}",
        consts::RELEASES_URL,
        target_version,
        archive_name
    );
    // Create a temp file to download the archive
    let mut archived_tempfile = NamedTempFile::new().into_diagnostic()?;

    let client = Client::new();
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
    use crate::cli::self_update::ReleaseNotesFormatter;
    use std::path::PathBuf;

    #[test]
    pub fn test_markdown_section_detection() {
        // Test that formatter correctly identifies correct and improper markdown headers
        assert_eq!(
            ReleaseNotesFormatter::extract_section_name("# Header 1"),
            Some("Header 1")
        );
        assert_eq!(
            ReleaseNotesFormatter::extract_section_name("# Header#"),
            Some("Header#")
        );
        assert_eq!(
            ReleaseNotesFormatter::extract_section_name("## Header 2"),
            Some("Header 2")
        );
        assert_eq!(
            ReleaseNotesFormatter::extract_section_name(" ## Header 3"),
            Some("Header 3")
        );
        assert_eq!(
            ReleaseNotesFormatter::extract_section_name("   ## Almost Code Block"),
            Some("Almost Code Block")
        );

        assert_eq!(
            ReleaseNotesFormatter::extract_section_name("###Header"),
            None
        );
        assert_eq!(
            ReleaseNotesFormatter::extract_section_name("Header 3# Header"),
            None
        );
        assert_eq!(
            ReleaseNotesFormatter::extract_section_name("    # Code Block"),
            None
        );
    }

    #[test]
    pub fn test_compare_release_notes() {
        // Test that the formatter correctly skips sections and formats the release notes
        let markdown = r#"#### Highlights
- World peace
- Bread will no longer fall butter-side down

#### Changed
- The sky is now green
- Water is now dry

#### New Contributors
- @alice (Alice)
- @bob (Bob)

#### Download pixi v1.2.3
No one knows what a markdown table looks like by heart
Let's just say it's a table"#;

        fn append_line(expected: &mut String, line: &str, style: Option<&console::Style>) {
            let styled_line = match style {
                Some(style) => style.apply_to(line).to_string(),
                None => line.to_string(),
            };
            expected.push_str(&styled_line);
            expected.push('\n');
        }
        let mut expected = String::new();
        let yellow = &console::Style::new().yellow();
        let cyan = &console::Style::new().cyan();
        append_line(&mut expected, "#### Highlights", Some(cyan));
        append_line(&mut expected, "- World peace", Some(yellow));
        append_line(
            &mut expected,
            "- Bread will no longer fall butter-side down",
            Some(yellow),
        );
        append_line(&mut expected, "", None);
        append_line(&mut expected, "#### Changed", Some(cyan));
        append_line(&mut expected, "- The sky is now green", Some(yellow));
        append_line(&mut expected, "- Water is now dry", Some(yellow));
        append_line(&mut expected, "", None);

        let formatter = ReleaseNotesFormatter::new_from_string(markdown);
        let formatted = formatter.get_formatted_notes();

        // Ensure same number of lines (zip will stop at the shortest)
        assert_eq!(
            expected.lines().count(),
            formatted.lines().count(),
            "Line count differs"
        );

        // assert line by line to get a better error message
        for (i, (expected_line, formatted_line)) in
            expected.lines().zip(formatted.lines()).enumerate()
        {
            assert_eq!(expected_line, formatted_line, "Line {} differs", i + 1);
        }
    }

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
