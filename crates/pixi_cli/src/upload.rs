use std::path::PathBuf;

use clap::Parser;
use miette::IntoDiagnostic;
use rattler_upload::upload::opt::{CommonOpts, PrefixOpts, ServerType, UploadOpts};
use url::Url;

/// Upload a conda package
///
/// With this command, you can upload a conda package to a channel.
///
/// Examples:
///   pixi upload prefix --channel my_channel my_package.conda
///   pixi upload <https://prefix.dev/api/v1/upload/my_channel> my_package.conda  (legacy)
///
/// Use `pixi auth login` to authenticate with the server.
#[derive(Parser, Debug)]
#[command(trailing_var_arg = true)]
pub struct Args {
    /// Arguments for the upload command
    #[arg(allow_hyphen_values = true)]
    args: Vec<String>,
}

/// Upload a package to a channel
pub async fn execute(args: Args) -> miette::Result<()> {
    // Try to detect if this is the legacy format (URL as first argument)
    let upload_opts = if !args.args.is_empty() && is_legacy_format(&args.args[0]) {
        // Legacy format: pixi upload <url> <package_file>
        parse_legacy_format(args.args)?
    } else {
        // New format: parse using rattler_upload's UploadOpts
        match UploadOpts::try_parse_from(
            std::iter::once("upload").chain(args.args.iter().map(|s| s.as_str())),
        ) {
            Ok(opts) => opts,
            Err(e) => {
                // If parsing fails, check if it might be legacy format without http(s)
                if !args.args.is_empty() {
                    eprintln!("Failed to parse upload command: {}", e);
                    eprintln!();
                    eprintln!("Note: The upload command format has changed. Examples:");
                    eprintln!("  pixi upload prefix --channel my_channel my_package.conda");
                    eprintln!(
                        "  pixi upload quetz --url https://quetz.example.com --channel my_channel my_package.conda"
                    );
                    eprintln!("  pixi upload anaconda --owner my_user my_package.conda");
                    eprintln!();
                    eprintln!("For backwards compatibility, the legacy format is still supported:");
                    eprintln!(
                        "  pixi upload https://prefix.dev/api/v1/upload/my_channel my_package.conda"
                    );
                }
                return Err(e).into_diagnostic();
            }
        }
    };

    // Execute the upload using rattler_upload
    rattler_upload::upload_from_args(upload_opts).await
}

/// Check if the first argument looks like a legacy URL format
fn is_legacy_format(arg: &str) -> bool {
    // Check if it's a URL (starts with http:// or https://)
    if let Ok(url) = Url::parse(arg) {
        if matches!(url.scheme(), "http" | "https") {
            // Additional check: does it look like an upload URL?
            let path = url.path();
            return path.contains("/upload") || path.contains("/api/");
        }
    }
    false
}

/// Parse the legacy format and convert to UploadOpts
fn parse_legacy_format(args: Vec<String>) -> miette::Result<UploadOpts> {
    if args.len() < 2 {
        return Err(miette::miette!(
            "Legacy format requires at least 2 arguments: <url> <package_file>"
        ));
    }

    let url_str = &args[0];
    let package_files: Vec<PathBuf> = args[1..].iter().map(PathBuf::from).collect();

    let url = Url::parse(url_str)
        .into_diagnostic()
        .map_err(|e| miette::miette!("Failed to parse URL '{}': {}", url_str, e))?;

    // Try to detect the server type and extract the channel from the URL
    let (server_type, detected_url, channel) = detect_server_type_from_url(&url)?;

    eprintln!("Using legacy upload format. Consider using the new format:");
    eprintln!(
        "  pixi upload {} --channel {} {}",
        server_type,
        channel,
        package_files
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(" ")
    );
    eprintln!();

    // Create UploadOpts based on detected server type
    let server_type = match server_type.as_str() {
        "prefix" => {
            let prefix_opts = PrefixOpts {
                url: detected_url,
                channel,
                api_key: None,
                attestation: None,
                skip_existing: false,
            };
            ServerType::Prefix(prefix_opts)
        }
        _ => {
            return Err(miette::miette!(
                "Could not determine server type from URL. Please use the new upload format."
            ));
        }
    };

    Ok(UploadOpts {
        package_files,
        server_type,
        common: CommonOpts {
            output_dir: None,
            use_zstd: true,
            use_bz2: true,
            experimental: false,
            allow_insecure_host: None,
            auth_file: None,
            channel_priority: None,
        },
        auth_store: None,
    })
}

/// Detect the server type from the URL
fn detect_server_type_from_url(url: &Url) -> miette::Result<(String, Url, String)> {
    let host = url
        .host_str()
        .ok_or_else(|| miette::miette!("URL has no host"))?;

    let path = url.path();

    // Detect prefix.dev URLs
    if host.contains("prefix.dev") {
        // Extract channel from path like /api/v1/upload/my_channel
        let channel = extract_channel_from_path(path)?;

        // Reconstruct base URL
        let base_url = Url::parse(&format!("{}://{}", url.scheme(), host)).into_diagnostic()?;

        return Ok(("prefix".to_string(), base_url, channel));
    }

    // Could add more server type detection here (quetz, artifactory, etc.)

    Err(miette::miette!(
        "Could not detect server type from URL: {}. Supported hosts: prefix.dev",
        url
    ))
}

/// Extract channel name from a path like /api/v1/upload/my_channel
fn extract_channel_from_path(path: &str) -> miette::Result<String> {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();

    // Look for the part after "upload"
    if let Some(upload_idx) = parts.iter().position(|&p| p == "upload") {
        if upload_idx + 1 < parts.len() {
            return Ok(parts[upload_idx + 1].to_string());
        }
    }

    Err(miette::miette!(
        "Could not extract channel from path: {}. Expected format: /api/v1/upload/<channel>",
        path
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_legacy_format() {
        assert!(is_legacy_format(
            "https://prefix.dev/api/v1/upload/my_channel"
        ));
        assert!(is_legacy_format("http://localhost:8000/upload/test"));
        assert!(!is_legacy_format("prefix"));
        assert!(!is_legacy_format("my_package.conda"));
    }

    #[test]
    fn test_extract_channel() {
        assert_eq!(
            extract_channel_from_path("/api/v1/upload/my_channel").unwrap(),
            "my_channel"
        );
        assert_eq!(extract_channel_from_path("/upload/test").unwrap(), "test");
        assert!(extract_channel_from_path("/api/v1/").is_err());
    }

    #[test]
    fn test_detect_server_type_prefix() {
        let url = Url::parse("https://prefix.dev/api/v1/upload/my_channel").unwrap();
        let (server_type, base_url, channel) = detect_server_type_from_url(&url).unwrap();

        assert_eq!(server_type, "prefix");
        assert_eq!(base_url.as_str(), "https://prefix.dev/");
        assert_eq!(channel, "my_channel");
    }
}
