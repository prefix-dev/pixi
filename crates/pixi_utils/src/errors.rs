use thiserror::Error;

#[derive(Error, Debug)]
pub enum HashError {
    #[error("Hash verification failed: Empty {algorithm} hash provided for {package_name}")]
    EmptyHash {
        algorithm: String,
        package_name: String,
    },
    #[error(
        "Hash verification failed: Invalid {algorithm} hash for {package_name}: not a valid hex string"
    )]
    InvalidHex {
        algorithm: String,
        package_name: String,
    },
    #[error(
        "Hash verification failed: Invalid {algorithm} hash length for {package_name}: expected {expected} characters, got {actual}"
    )]
    InvalidLength {
        algorithm: String,
        package_name: String,
        expected: usize,
        actual: usize,
    },
    #[error(
        "Hash verification failed: Could not parse {algorithm} hash for {package_name}: {hash_str}"
    )]
    ParseFailed {
        algorithm: String,
        package_name: String,
        hash_str: String,
    },
    #[error(
        "Hash verification failed: Unsupported hash algorithm '{algorithm}' for {package_name}. Only SHA256 and MD5 are supported."
    )]
    UnsupportedAlgorithm {
        algorithm: String,
        package_name: String,
    },
}
