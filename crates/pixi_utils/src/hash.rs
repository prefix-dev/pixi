use rattler_digest::{Md5, Sha256, parse_digest_from_hex};
use rattler_lock::PackageHashes;
use std::fmt::Display;

/// Validates and parses a hash string into PackageHashes
///
/// # Arguments
/// * `algorithm` - The hash algorithm (e.g., "sha256", "md5")
/// * `hash_str` - The hash value as a hex string
/// * `package_name` - The package name for error messages
///
/// # Returns
/// * `Ok(PackageHashes)` if the hash is valid
/// * `Err(String)` with a descriptive error message if invalid
pub fn validate_and_parse_hash(
    algorithm: &str,
    hash_str: &str,
    package_name: &impl Display,
) -> Result<PackageHashes, String> {
    // Check empty hash
    if hash_str.is_empty() {
        return Err(format!(
            "Hash verification failed: Empty {} hash provided for {}",
            algorithm.to_uppercase(),
            package_name
        ));
    }

    // Check hex validity
    if !hash_str.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "Hash verification failed: Invalid {} hash for {}: not a valid hex string",
            algorithm.to_uppercase(),
            package_name
        ));
    }

    // Parse based on algorithm
    match algorithm {
        "sha256" => {
            if hash_str.len() != 64 {
                return Err(format!(
                    "Hash verification failed: Invalid SHA256 hash for {}: expected 64 characters, got {}",
                    package_name,
                    hash_str.len()
                ));
            }
            parse_digest_from_hex::<Sha256>(hash_str)
                .map(PackageHashes::Sha256)
                .ok_or_else(|| {
                    format!(
                        "Hash verification failed: Invalid SHA256 hash for {}: {}",
                        package_name, hash_str
                    )
                })
        }
        "md5" => {
            if hash_str.len() != 32 {
                return Err(format!(
                    "Hash verification failed: Invalid MD5 hash for {}: expected 32 characters, got {}",
                    package_name,
                    hash_str.len()
                ));
            }
            parse_digest_from_hex::<Md5>(hash_str)
                .map(PackageHashes::Md5)
                .ok_or_else(|| {
                    format!(
                        "Hash verification failed: Invalid MD5 hash for {}: {}",
                        package_name, hash_str
                    )
                })
        }
        _ => unreachable!(
            "validate_and_parse_hash called with unsupported algorithm: {}",
            algorithm
        ),
    }
}

/// Extracts and parses a hash from a URL fragment
///
/// Parses fragments like "#sha256=abc123" or "#md5=def456&egg=foo"
///
/// # Arguments
/// * `fragment` - The URL fragment (without the leading #)
/// * `package_name` - The package name for error messages
///
/// # Returns
/// * `Ok(Some(PackageHashes))` if a valid hash is found
/// * `Ok(None)` if no hash parameter is found
/// * `Err(String)` if a hash parameter is found but invalid
pub fn parse_hash_from_url_fragment(
    fragment: &str,
    package_name: &impl Display,
) -> Result<Option<PackageHashes>, String> {
    // Find hash parameter in fragment
    for param in fragment.split('&') {
        if let Some((algorithm, hash_str)) = param.split_once('=') {
            let algorithm_lower = algorithm.to_lowercase();

            // Check if this parameter is a hash algorithm
            match algorithm_lower.as_str() {
                "sha256" | "md5" => {
                    return validate_and_parse_hash(&algorithm_lower, hash_str, package_name)
                        .map(Some);
                }
                alg if alg.contains("sha") || alg.contains("md5") || alg.contains("blake") => {
                    return Err(format!(
                        "Hash verification failed: Unsupported hash algorithm '{}' for {}. Only SHA256 and MD5 are supported.",
                        algorithm_lower, package_name
                    ));
                }
                _ => continue, // Not a hash parameter
            }
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hash_from_url_fragment() {
        // Valid SHA256
        let result = parse_hash_from_url_fragment(
            "sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            &"test-package",
        )
        .unwrap();
        assert!(matches!(result, Some(PackageHashes::Sha256(_))));

        // Valid MD5
        let result =
            parse_hash_from_url_fragment("md5=d41d8cd98f00b204e9800998ecf8427e", &"test-package")
                .unwrap();
        assert!(matches!(result, Some(PackageHashes::Md5(_))));

        // Multiple parameters with hash
        let result = parse_hash_from_url_fragment(
            "egg=foo&sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855&bar=baz",
            &"test-package"
        ).unwrap();
        assert!(matches!(result, Some(PackageHashes::Sha256(_))));

        // No hash parameter
        let result = parse_hash_from_url_fragment("egg=foo&bar=baz", &"test-package").unwrap();
        assert!(result.is_none());

        // Invalid hash algorithm
        let result = parse_hash_from_url_fragment("sha512=abcdef", &"test-package");
        assert!(result.is_err());

        // Empty hash
        let result = parse_hash_from_url_fragment("sha256=", &"test-package");
        assert!(result.is_err());

        // Invalid hex
        let result = parse_hash_from_url_fragment("sha256=xyz", &"test-package");
        assert!(result.is_err());
    }
}
