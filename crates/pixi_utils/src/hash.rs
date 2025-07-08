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

/// Updates or adds a hash parameter to a URL fragment while preserving other parameters
///
/// For example:
/// - "egg=foo" + sha256 hash -> "egg=foo&sha256=abc123"
/// - "sha256=old&egg=foo" + new sha256 -> "sha256=new&egg=foo"
/// - "" + sha256 hash -> "sha256=abc123"
pub fn update_fragment_with_hash(fragment: Option<&str>, hash: &PackageHashes) -> String {
    // Convert hash to fragment format
    let hash_param = match hash {
        PackageHashes::Sha256(sha256) => format!("sha256={:x}", sha256),
        PackageHashes::Md5(md5) => format!("md5={:x}", md5),
        PackageHashes::Md5Sha256(_md5, sha256) => {
            // Prefer SHA256 for direct URLs
            format!("sha256={:x}", sha256)
        }
    };

    match fragment {
        None | Some("") => hash_param,
        Some(existing) => {
            // Parse existing parameters
            let mut params: Vec<String> = existing.split('&').map(|s| s.to_string()).collect();
            let mut hash_updated = false;

            // Update existing hash parameter if present
            for param in &mut params {
                if param.starts_with("sha256=") || param.starts_with("md5=") {
                    *param = hash_param.clone();
                    hash_updated = true;
                    break;
                }
            }

            // If no hash parameter was found, add it
            if !hash_updated {
                params.push(hash_param);
            }

            params.join("&")
        }
    }
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

    #[test]
    fn test_update_fragment_with_hash() {
        let sha256_hash = parse_digest_from_hex::<Sha256>(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        )
        .unwrap();
        let md5_hash = parse_digest_from_hex::<Md5>("d41d8cd98f00b204e9800998ecf8427e").unwrap();

        // Test 1: No existing fragment
        let result = update_fragment_with_hash(None, &PackageHashes::Sha256(sha256_hash));
        assert_eq!(
            result,
            "sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        // Test 2: Empty fragment
        let result = update_fragment_with_hash(Some(""), &PackageHashes::Sha256(sha256_hash));
        assert_eq!(
            result,
            "sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        // Test 3: Fragment with egg parameter
        let result =
            update_fragment_with_hash(Some("egg=mypackage"), &PackageHashes::Sha256(sha256_hash));
        assert_eq!(
            result,
            "egg=mypackage&sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        // Test 4: Fragment with multiple parameters
        let result = update_fragment_with_hash(
            Some("egg=mypackage&subdirectory=src"),
            &PackageHashes::Sha256(sha256_hash),
        );
        assert_eq!(
            result,
            "egg=mypackage&subdirectory=src&sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        // Test 5: Fragment with existing sha256 hash
        let result = update_fragment_with_hash(
            Some("sha256=oldhash&egg=mypackage"),
            &PackageHashes::Sha256(sha256_hash),
        );
        assert_eq!(
            result,
            "sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855&egg=mypackage"
        );

        // Test 6: Fragment with existing md5 hash, updating with sha256
        let result = update_fragment_with_hash(
            Some("md5=oldhash&egg=mypackage"),
            &PackageHashes::Sha256(sha256_hash),
        );
        assert_eq!(
            result,
            "sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855&egg=mypackage"
        );

        // Test 7: MD5 hash
        let result =
            update_fragment_with_hash(Some("egg=mypackage"), &PackageHashes::Md5(md5_hash));
        assert_eq!(result, "egg=mypackage&md5=d41d8cd98f00b204e9800998ecf8427e");

        // Test 8: Md5Sha256 hash (should prefer SHA256)
        let result = update_fragment_with_hash(
            Some("egg=mypackage"),
            &PackageHashes::Md5Sha256(md5_hash, sha256_hash),
        );
        assert_eq!(
            result,
            "egg=mypackage&sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
