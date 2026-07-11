//! Client for the remote build (artifact) cache hosted on prefix.dev.
//!
//! The remote cache mirrors the local [`crate::cache::ArtifactCache`]: built
//! `.conda` artifacts are stored under an opaque structural cache key and are
//! discoverable through *source hashes* that capture the state of the source
//! tree they were built from. Two kinds of source hashes exist:
//!
//! * `git:<tree-hash>` — the git tree object hash of the source directory
//!   when the working tree is clean. This can be computed before a build (and
//!   before ever talking to a build backend), enabling a single-round-trip
//!   cache probe. A tree hash only vouches for tracked, in-tree files, so a
//!   `git:` index match is a *pre-filter*: before downloading, the input
//!   files the tree cannot vouch for (paths outside the source directory
//!   such as `../recipe.yaml`, and untracked files) are verified against the
//!   entry's recorded fingerprints.
//! * `src:<digest>` — a blake3 digest over the build's input files (relative
//!   path plus blake3 content hash of each file, as recorded in the local
//!   sidecar). Because the input file set is only known from a previous build
//!   (the backend reports the input globs), a fresh checkout matches this
//!   mode by listing the candidate entries — whose metadata carries the glob
//!   sets and per-file fingerprints — and re-evaluating them locally.
//!
//! The [`RemoteEntryMetadata`] document attached to every entry is therefore
//! a required part of the contract, not an optimization: it records *which
//! files are part of the cache entry* (the backend-reported input glob sets
//! plus the per-file fingerprint map), and it is the only way a machine that
//! never built the package can reconstruct the input file set.
//!
//! All requests go through the authenticated [`LazyClient`], so credentials
//! stored for prefix.dev via `pixi auth login` are attached automatically.

use std::collections::BTreeMap;
use std::path::Path;

use pixi_build_types::InputGlobSet;
use pixi_path::AbsPathBuf;
use rattler_conda_types::RepoDataRecord;
use rattler_digest::{Sha256, Sha256Hash, digest::Digest as _, parse_digest_from_hex};
use rattler_networking::LazyClient;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncWriteExt as _;
use url::Url;

/// Settings for the remote build cache, registered in the compute engine's
/// global [`pixi_compute_engine::DataStore`] when the feature is enabled.
#[derive(Debug, Clone)]
pub struct RemoteBuildCacheSettings {
    /// Base URL of the server hosting the cache (e.g. `https://prefix.dev`).
    pub url: Url,

    /// The owner of the cache. Defaults to `me`, which the server resolves
    /// to the authenticated user. Kept configurable so organization-owned
    /// caches can be addressed later without a client change.
    pub owner: String,

    /// Whether artifacts built locally should be uploaded to the remote
    /// cache after a successful build.
    pub write: bool,
}

/// Errors produced by remote build cache operations.
///
/// These errors never fail a build: callers log them and fall back to
/// building from source (or skip the upload).
#[derive(Debug, Error)]
pub enum RemoteBuildCacheError {
    #[error("failed to construct request URL")]
    InvalidUrl,

    #[error("request to remote build cache failed")]
    Request(#[from] reqwest_middleware::Error),

    #[error("remote build cache returned an error")]
    Response(#[from] reqwest::Error),

    #[error("failed to read or write a local file")]
    Io(#[from] std::io::Error),

    #[error("downloaded artifact has sha256 {computed} but the cache entry declares {expected}")]
    Sha256Mismatch { expected: String, computed: String },

    #[error("the cache entry declares an invalid sha256 hash")]
    InvalidSha256,

    #[error("failed to store the downloaded artifact in the local artifact cache")]
    LocalCache(#[source] crate::cache::ArtifactCacheError),

    #[error("failed to evaluate the entry's input globs locally")]
    Glob(#[source] std::sync::Arc<pixi_glob::GlobSetError>),

    #[error("the downloaded artifact is not usable: {0}")]
    InvalidArtifact(String),

    #[error("a background task panicked")]
    Join(#[from] tokio::task::JoinError),
}

/// The metadata document stored with every remote cache entry.
///
/// This is opaque to the server; the client uses it to re-validate a
/// candidate entry against the local source tree (path-dependency mode) and
/// to reconstruct the local sidecar after a download.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteEntryMetadata {
    /// The input glob sets the backend reported for the build.
    pub input_globs: Vec<InputGlobSet>,

    /// blake3 content hash per input file, keyed by the file's path relative
    /// to the source directory (`/`-separated, `..` components allowed for
    /// inputs outside the source directory).
    pub input_files: BTreeMap<String, String>,

    /// The repodata record synthesized from the artifact at build time.
    /// Used as a hint only; clients re-synthesize the record from the
    /// downloaded artifact.
    pub record: RepoDataRecord,
}

/// A cache entry as returned by the query endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteEntrySummary {
    pub id: i64,
    pub cache_key: String,
    pub artifact_sha256: String,
    pub artifact_size: i64,
    pub artifact_filename: String,
}

/// A cache entry as returned by the listing endpoint, including the metadata
/// document needed for local re-validation.
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteEntryDetails {
    #[serde(flatten)]
    pub summary: RemoteEntrySummary,
    pub hashes: Vec<String>,
    pub metadata: serde_json::Value,
}

impl RemoteEntryDetails {
    /// Parses the opaque metadata document. Entries written by other (newer
    /// or older) clients may not parse; those are skipped by callers.
    pub fn parsed_metadata(&self) -> Option<RemoteEntryMetadata> {
        serde_json::from_value(self.metadata.clone()).ok()
    }
}

#[derive(Debug, Serialize)]
struct QueryRequest<'a> {
    queries: Vec<QueryItem<'a>>,
}

#[derive(Debug, Serialize)]
struct QueryItem<'a> {
    cache_key: &'a str,
    hashes: &'a [String],
}

#[derive(Debug, Deserialize)]
struct QueryResponse {
    results: Vec<QueryResult>,
}

#[derive(Debug, Deserialize)]
struct QueryResult {
    #[allow(dead_code)]
    cache_key: String,
    matches: Vec<RemoteEntrySummary>,
}

#[derive(Debug, Deserialize)]
struct ListEntriesResponse {
    entries: Vec<RemoteEntryDetails>,
}

#[derive(Debug, Serialize)]
struct RegisterEntryRequest<'a> {
    artifact_sha256: String,
    artifact_filename: &'a str,
    hashes: &'a [String],
    metadata: &'a RemoteEntryMetadata,
}

/// A thin, authenticated HTTP client for the remote build cache API.
#[derive(Clone)]
pub struct RemoteBuildCacheClient {
    settings: RemoteBuildCacheSettings,
    client: LazyClient,
}

impl RemoteBuildCacheClient {
    pub fn new(settings: RemoteBuildCacheSettings, client: LazyClient) -> Self {
        Self { settings, client }
    }

    /// Whether locally built artifacts should be uploaded.
    pub fn write_enabled(&self) -> bool {
        self.settings.write
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url, RemoteBuildCacheError> {
        let mut url = self.settings.url.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| RemoteBuildCacheError::InvalidUrl)?;
            path.pop_if_empty();
            path.extend(["api", "v1", "build-cache", self.settings.owner.as_str()]);
            path.extend(segments);
        }
        Ok(url)
    }

    /// Queries the hash index: returns the entries of `cache_key` registered
    /// under at least one of `hashes`.
    pub async fn query(
        &self,
        cache_key: &str,
        hashes: &[String],
    ) -> Result<Vec<RemoteEntrySummary>, RemoteBuildCacheError> {
        if hashes.is_empty() {
            return Ok(Vec::new());
        }
        let url = self.endpoint(&["query"])?;
        let response: QueryResponse = self
            .client
            .client()
            .post(url)
            .json(&QueryRequest {
                queries: vec![QueryItem { cache_key, hashes }],
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(response
            .results
            .into_iter()
            .next()
            .map(|result| result.matches)
            .unwrap_or_default())
    }

    /// Lists the entries stored for `cache_key`, including their metadata.
    pub async fn entries(
        &self,
        cache_key: &str,
    ) -> Result<Vec<RemoteEntryDetails>, RemoteBuildCacheError> {
        let url = self.endpoint(&["entries", cache_key])?;
        let response: ListEntriesResponse = self
            .client
            .client()
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(response.entries)
    }

    /// Downloads the artifact of `(cache_key, sha256)` to `destination`,
    /// verifying the content hash while streaming.
    pub async fn download_artifact(
        &self,
        cache_key: &str,
        sha256: &str,
        destination: &Path,
    ) -> Result<(), RemoteBuildCacheError> {
        let expected: Sha256Hash =
            parse_digest_from_hex::<Sha256>(sha256).ok_or(RemoteBuildCacheError::InvalidSha256)?;
        let url = self.endpoint(&["artifact", cache_key, sha256])?;
        let mut response = self
            .client
            .client()
            .get(url)
            .send()
            .await?
            .error_for_status()?;

        let mut file = tokio::fs::File::create(destination).await?;
        let mut hasher = Sha256::new();
        while let Some(chunk) = response.chunk().await? {
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;

        let computed = hasher.finalize();
        if computed != expected {
            return Err(RemoteBuildCacheError::Sha256Mismatch {
                expected: sha256.to_string(),
                computed: hex::encode(computed),
            });
        }
        Ok(())
    }

    /// Uploads an artifact blob. The blob only becomes discoverable once
    /// [`register_entry`](Self::register_entry) is called for it.
    pub async fn upload_artifact(
        &self,
        artifact: &Path,
        sha256: &Sha256Hash,
        file_name: &str,
    ) -> Result<(), RemoteBuildCacheError> {
        let url = self.endpoint(&["artifact"])?;
        let file = tokio::fs::File::open(artifact).await?;
        let size = file.metadata().await?.len();
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
        self.client
            .client()
            .put(url)
            .header("X-File-Name", file_name)
            .header("X-File-SHA256", hex::encode(sha256))
            .header(reqwest::header::CONTENT_LENGTH, size)
            .body(body)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Registers a cache entry for a previously uploaded artifact.
    pub async fn register_entry(
        &self,
        cache_key: &str,
        sha256: &Sha256Hash,
        file_name: &str,
        hashes: &[String],
        metadata: &RemoteEntryMetadata,
    ) -> Result<(), RemoteBuildCacheError> {
        let url = self.endpoint(&["entries", cache_key])?;
        self.client
            .client()
            .put(url)
            .json(&RegisterEntryRequest {
                artifact_sha256: hex::encode(sha256),
                artifact_filename: file_name,
                hashes,
                metadata,
            })
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

/// Computes the `git:` source hash for `source_dir`, or `None` when the
/// directory is not inside a git repository, git is unavailable, or the
/// working tree under `source_dir` has uncommitted or untracked changes.
///
/// The hash is the git *tree object* hash of the source directory at `HEAD`,
/// not the commit hash: unrelated commits elsewhere in the repository do not
/// invalidate the cache entry.
pub fn git_source_hash(source_dir: &Path) -> Option<String> {
    let git = which::which("git").ok()?;

    let run = |args: &[&str]| -> Option<String> {
        let output = std::process::Command::new(&git)
            .arg("-C")
            .arg(source_dir)
            .args(args)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };

    // Any modified, staged, or untracked file below the source directory
    // makes the tree state unrepresentable by a git hash.
    let status = run(&[
        "status",
        "--porcelain",
        "--untracked-files=normal",
        "--",
        ".",
    ])?;
    if !status.is_empty() {
        tracing::debug!(
            source_dir = %source_dir.display(),
            "source tree is dirty, not using a git source hash"
        );
        return None;
    }

    // The path of the source directory relative to the repository root;
    // empty when the source directory *is* the root. `HEAD:<prefix>`
    // resolves to the tree object of that directory (`HEAD:` alone is the
    // root tree).
    let prefix = run(&["rev-parse", "--show-prefix"])?;
    let tree = run(&["rev-parse", &format!("HEAD:{prefix}")])?;
    if tree.is_empty() {
        return None;
    }
    Some(format!("git:{tree}"))
}

/// Returns the set of git-tracked files below `source_dir`, as
/// `/`-separated paths relative to `source_dir`, or `None` when the
/// directory is not inside a git repository (or git is unavailable).
///
/// A matching tree hash only vouches for these files: input files outside
/// the source directory or untracked (e.g. gitignored but glob-matched)
/// files can differ between two checkouts with identical tree hashes, so
/// they must be verified by content before a `git:` index match is trusted.
pub fn git_tracked_files(source_dir: &Path) -> Option<std::collections::BTreeSet<String>> {
    let git = which::which("git").ok()?;
    let output = std::process::Command::new(git)
        .arg("-C")
        .arg(source_dir)
        .args(["ls-files", "-z", "--", "."])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect(),
    )
}

/// Verifies the input files that a matching git tree hash does *not* vouch
/// for — files outside the source directory (`../` paths) and untracked
/// files — against the fingerprints recorded in a cache entry.
///
/// `local_files` is the locally glob-matched input set, `recorded` the
/// entry's relative-path → blake3 map, and `tracked` the output of
/// [`git_tracked_files`] (pass `None` to treat every file as uncovered,
/// which degrades to a full content comparison).
///
/// Returns `false` when the uncovered subsets differ in membership or any
/// uncovered file's content hash disagrees. In the common case — every
/// input tracked and in-tree — nothing needs hashing at all.
pub fn verify_uncovered_fingerprints(
    source_dir: &Path,
    local_files: impl IntoIterator<Item = AbsPathBuf>,
    recorded: &BTreeMap<String, String>,
    tracked: Option<&std::collections::BTreeSet<String>>,
) -> std::io::Result<bool> {
    let covered = |rel: &str| {
        !rel.starts_with("../")
            && tracked
                .map(|tracked| tracked.contains(rel))
                .unwrap_or(false)
    };

    // Relativize the local input set without hashing anything yet.
    let mut local_uncovered = BTreeMap::new();
    for file in local_files {
        let file = file.as_std_path().to_path_buf();
        let relative = pathdiff::diff_paths(&file, source_dir).ok_or_else(|| {
            std::io::Error::other(format!(
                "cannot express {} relative to {}",
                file.display(),
                source_dir.display()
            ))
        })?;
        let normalized = relative.to_string_lossy().replace('\\', "/");
        if !covered(&normalized) {
            local_uncovered.insert(normalized, file);
        }
    }

    // Membership must agree: an uncovered file that appeared locally but was
    // not recorded (or vice versa) means a different source state.
    let recorded_uncovered: Vec<&String> = recorded
        .keys()
        .filter(|rel| !covered(rel.as_str()))
        .collect();
    if local_uncovered.len() != recorded_uncovered.len()
        || !recorded_uncovered
            .iter()
            .all(|rel| local_uncovered.contains_key(rel.as_str()))
    {
        return Ok(false);
    }

    // Contents of every uncovered file must match the recorded fingerprint.
    for (rel, file) in &local_uncovered {
        let expected = recorded
            .get(rel)
            .expect("membership was checked above; the key must exist");
        if crate::cache::blake3_file(file)? != *expected {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Computes the relative-path → blake3 fingerprint map for a set of input
/// files, mirroring what the local sidecar stores but keyed by portable
/// relative paths so the map is comparable across machines.
pub fn fingerprint_input_files(
    source_dir: &Path,
    input_files: impl IntoIterator<Item = AbsPathBuf>,
) -> std::io::Result<BTreeMap<String, String>> {
    let mut fingerprints = BTreeMap::new();
    for file in input_files {
        let file = file.as_std_path();
        let relative = pathdiff::diff_paths(file, source_dir).ok_or_else(|| {
            std::io::Error::other(format!(
                "cannot express {} relative to {}",
                file.display(),
                source_dir.display()
            ))
        })?;
        let normalized = relative.to_string_lossy().replace('\\', "/");
        let content = crate::cache::blake3_file(file)?;
        fingerprints.insert(normalized, content);
    }
    Ok(fingerprints)
}

/// Computes the `src:` source hash from a fingerprint map: a blake3 digest
/// over the sorted `(relative path, content hash)` pairs. `BTreeMap`
/// iteration order makes the digest independent of discovery order.
pub fn source_files_hash(fingerprints: &BTreeMap<String, String>) -> String {
    let mut hasher = blake3::Hasher::new();
    for (path, content) in fingerprints {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update(content.as_bytes());
        hasher.update(b"\n");
    }
    format!("src:{}", hasher.finalize().to_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_files_hash_is_order_independent_and_content_sensitive() {
        let mut a = BTreeMap::new();
        a.insert("src/main.py".to_string(), "aaaa".to_string());
        a.insert("../recipe.yaml".to_string(), "bbbb".to_string());

        // Same entries inserted in the opposite order.
        let mut b = BTreeMap::new();
        b.insert("../recipe.yaml".to_string(), "bbbb".to_string());
        b.insert("src/main.py".to_string(), "aaaa".to_string());

        assert_eq!(source_files_hash(&a), source_files_hash(&b));

        // Changing a content hash changes the digest.
        b.insert("src/main.py".to_string(), "cccc".to_string());
        assert_ne!(source_files_hash(&a), source_files_hash(&b));

        // The digest carries the `src:` namespace.
        assert!(source_files_hash(&a).starts_with("src:"));
    }

    #[test]
    fn source_files_hash_separates_path_and_content() {
        // `("ab", "c")` and `("a", "bc")` must not collide.
        let mut a = BTreeMap::new();
        a.insert("ab".to_string(), "c".to_string());
        let mut b = BTreeMap::new();
        b.insert("a".to_string(), "bc".to_string());
        assert_ne!(source_files_hash(&a), source_files_hash(&b));
    }

    #[test]
    fn fingerprint_input_files_relativizes_and_normalizes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let source = tmp.path().join("pkg");
        fs_err::create_dir_all(source.join("src")).unwrap();
        fs_err::write(source.join("src").join("main.py"), b"print(1)").unwrap();
        fs_err::write(tmp.path().join("recipe.yaml"), b"recipe").unwrap();

        let files = vec![
            AbsPathBuf::new(source.join("src").join("main.py")).unwrap(),
            AbsPathBuf::new(tmp.path().join("recipe.yaml")).unwrap(),
        ];
        let fingerprints = fingerprint_input_files(&source, files).unwrap();

        assert_eq!(
            fingerprints.keys().collect::<Vec<_>>(),
            vec!["../recipe.yaml", "src/main.py"],
        );
        // The values are blake3 hashes of the file contents.
        assert_eq!(
            fingerprints["src/main.py"],
            blake3::hash(b"print(1)").to_hex().to_string(),
        );
    }

    #[test]
    fn git_source_hash_of_non_repository_is_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(git_source_hash(tmp.path()), None);
    }

    #[test]
    fn verify_uncovered_fingerprints_logic() {
        use std::collections::BTreeSet;

        let tmp = tempfile::TempDir::new().unwrap();
        let source = tmp.path().join("pkg");
        fs_err::create_dir_all(&source).unwrap();
        fs_err::write(source.join("tracked.py"), b"tracked").unwrap();
        fs_err::write(source.join("generated.py"), b"generated").unwrap();
        fs_err::write(tmp.path().join("recipe.yaml"), b"recipe").unwrap();

        let local_files = || {
            vec![
                AbsPathBuf::new(source.join("tracked.py")).unwrap(),
                AbsPathBuf::new(source.join("generated.py")).unwrap(),
                AbsPathBuf::new(tmp.path().join("recipe.yaml")).unwrap(),
            ]
        };
        let tracked: BTreeSet<String> = BTreeSet::from(["tracked.py".to_string()]);

        let mut recorded = BTreeMap::new();
        // The tracked file's recorded fingerprint is bogus on purpose: the
        // tree hash vouches for it, so it must never be re-hashed.
        recorded.insert("tracked.py".to_string(), "not-checked".to_string());
        recorded.insert(
            "generated.py".to_string(),
            blake3::hash(b"generated").to_hex().to_string(),
        );
        recorded.insert(
            "../recipe.yaml".to_string(),
            blake3::hash(b"recipe").to_hex().to_string(),
        );

        // Uncovered files (untracked + out-of-tree) match → verified.
        assert!(
            verify_uncovered_fingerprints(&source, local_files(), &recorded, Some(&tracked))
                .unwrap()
        );

        // A changed out-of-tree file is caught.
        fs_err::write(tmp.path().join("recipe.yaml"), b"changed recipe").unwrap();
        assert!(
            !verify_uncovered_fingerprints(&source, local_files(), &recorded, Some(&tracked))
                .unwrap()
        );
        fs_err::write(tmp.path().join("recipe.yaml"), b"recipe").unwrap();

        // A changed untracked in-tree file is caught.
        fs_err::write(source.join("generated.py"), b"regenerated").unwrap();
        assert!(
            !verify_uncovered_fingerprints(&source, local_files(), &recorded, Some(&tracked))
                .unwrap()
        );
        fs_err::write(source.join("generated.py"), b"generated").unwrap();

        // An uncovered file recorded in the entry but missing locally is a
        // membership mismatch.
        let shorter = vec![
            AbsPathBuf::new(source.join("tracked.py")).unwrap(),
            AbsPathBuf::new(source.join("generated.py")).unwrap(),
        ];
        assert!(
            !verify_uncovered_fingerprints(&source, shorter, &recorded, Some(&tracked)).unwrap()
        );

        // Without tracking information everything is uncovered, so the bogus
        // fingerprint for `tracked.py` now fails the check.
        assert!(!verify_uncovered_fingerprints(&source, local_files(), &recorded, None).unwrap());
    }

    #[test]
    fn git_tracked_files_of_non_repository_is_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(git_tracked_files(tmp.path()), None);
    }

    #[test]
    fn git_source_hash_clean_vs_dirty() {
        let git = match which::which("git") {
            Ok(git) => git,
            // Without a git binary the production code also returns None,
            // so there is nothing further to test.
            Err(_) => return,
        };
        let run = |dir: &std::path::Path, args: &[&str]| {
            let status = std::process::Command::new(&git)
                .arg("-C")
                .arg(dir)
                .args(args)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@localhost")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@localhost")
                .output()
                .unwrap();
            assert!(status.status.success(), "git {args:?} failed");
        };

        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path();
        let pkg = repo.join("pkg");
        fs_err::create_dir_all(&pkg).unwrap();
        fs_err::write(pkg.join("main.py"), b"print(1)").unwrap();
        run(repo, &["init", "-q"]);
        run(repo, &["add", "."]);
        run(repo, &["commit", "-q", "-m", "initial"]);

        // Clean tree: a `git:`-namespaced tree hash for the subdirectory.
        let clean = git_source_hash(&pkg).expect("clean tree should have a hash");
        assert!(clean.starts_with("git:"), "{clean}");

        // The tracked-file listing is relative to the source directory.
        let tracked = git_tracked_files(&pkg).expect("repository should list tracked files");
        assert!(tracked.contains("main.py"), "{tracked:?}");

        // The subdirectory hash is the tree hash, not the commit hash: an
        // unrelated commit elsewhere must not change it.
        fs_err::write(repo.join("unrelated.txt"), b"other").unwrap();
        run(repo, &["add", "unrelated.txt"]);
        run(repo, &["commit", "-q", "-m", "unrelated"]);
        assert_eq!(git_source_hash(&pkg).as_ref(), Some(&clean));

        // An untracked file below the source dir makes it dirty.
        fs_err::write(pkg.join("scratch.py"), b"wip").unwrap();
        assert_eq!(git_source_hash(&pkg), None);
        fs_err::remove_file(pkg.join("scratch.py")).unwrap();

        // A modified tracked file makes it dirty too.
        fs_err::write(pkg.join("main.py"), b"print(2)").unwrap();
        assert_eq!(git_source_hash(&pkg), None);

        // Committing the change yields a new, different hash.
        run(repo, &["add", "."]);
        run(repo, &["commit", "-q", "-m", "change"]);
        let changed = git_source_hash(&pkg).expect("clean tree again");
        assert_ne!(changed, clean);
    }

    /// A minimal in-memory implementation of the server API, exercising the
    /// full client roundtrip: upload → register → query → list → download.
    #[tokio::test]
    async fn client_roundtrip_against_mock_server() {
        use std::sync::{Arc, Mutex};

        use axum::extract::{Path as AxumPath, State};
        use axum::routing::{get, post, put};
        use axum::{Json, Router};

        #[derive(Default, Clone)]
        struct ServerState {
            // sha256 -> bytes
            blobs: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
            // cache_key -> entries
            entries: Arc<Mutex<BTreeMap<String, Vec<serde_json::Value>>>>,
        }

        let state = ServerState::default();

        async fn put_artifact(
            State(state): State<ServerState>,
            headers: axum::http::HeaderMap,
            body: axum::body::Bytes,
        ) -> axum::http::StatusCode {
            let sha = headers["x-file-sha256"].to_str().unwrap().to_string();
            assert_eq!(
                headers["x-file-name"].to_str().unwrap(),
                "foo-1.0.0-h0_0.conda"
            );
            state.blobs.lock().unwrap().insert(sha, body.to_vec());
            axum::http::StatusCode::CREATED
        }

        async fn get_artifact(
            State(state): State<ServerState>,
            AxumPath((_key, sha)): AxumPath<(String, String)>,
        ) -> Vec<u8> {
            state.blobs.lock().unwrap().get(&sha).unwrap().clone()
        }

        async fn put_entry(
            State(state): State<ServerState>,
            AxumPath(key): AxumPath<String>,
            Json(mut body): Json<serde_json::Value>,
        ) -> axum::http::StatusCode {
            // Echo the server-side entry shape back.
            body["id"] = 1.into();
            body["cache_key"] = key.clone().into();
            body["artifact_size"] = 42.into();
            state
                .entries
                .lock()
                .unwrap()
                .entry(key)
                .or_default()
                .push(body);
            axum::http::StatusCode::CREATED
        }

        async fn get_entries(
            State(state): State<ServerState>,
            AxumPath(key): AxumPath<String>,
        ) -> Json<serde_json::Value> {
            let entries = state
                .entries
                .lock()
                .unwrap()
                .get(&key)
                .cloned()
                .unwrap_or_default();
            Json(serde_json::json!({ "entries": entries }))
        }

        async fn post_query(
            State(state): State<ServerState>,
            Json(body): Json<serde_json::Value>,
        ) -> Json<serde_json::Value> {
            let mut results = Vec::new();
            for query in body["queries"].as_array().unwrap() {
                let key = query["cache_key"].as_str().unwrap();
                let hashes: Vec<&str> = query["hashes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| h.as_str().unwrap())
                    .collect();
                let matches: Vec<serde_json::Value> = state
                    .entries
                    .lock()
                    .unwrap()
                    .get(key)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|entry| {
                        entry["hashes"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|hash| hashes.contains(&hash.as_str().unwrap()))
                    })
                    .collect();
                results.push(serde_json::json!({
                    "cache_key": key,
                    "matches": matches,
                }));
            }
            Json(serde_json::json!({ "results": results }))
        }

        let app = Router::new()
            .route("/api/v1/build-cache/me/artifact", put(put_artifact))
            .route(
                "/api/v1/build-cache/me/artifact/{key}/{sha}",
                get(get_artifact),
            )
            .route(
                "/api/v1/build-cache/me/entries/{key}",
                put(put_entry).get(get_entries),
            )
            .route("/api/v1/build-cache/me/query", post(post_query))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = RemoteBuildCacheClient::new(
            RemoteBuildCacheSettings {
                url: Url::parse(&format!("http://{addr}")).unwrap(),
                owner: "me".to_string(),
                write: true,
            },
            LazyClient::from(reqwest::Client::new()),
        );

        // Upload an artifact.
        let tmp = tempfile::TempDir::new().unwrap();
        let artifact = tmp.path().join("foo-1.0.0-h0_0.conda");
        fs_err::write(&artifact, b"pretend conda").unwrap();
        let sha256 = rattler_digest::compute_bytes_digest::<Sha256>(b"pretend conda");
        client
            .upload_artifact(&artifact, &sha256, "foo-1.0.0-h0_0.conda")
            .await
            .unwrap();

        // Register an entry under a git hash.
        let git_hash = "git:1234567890123456789012345678901234567890".to_string();
        let metadata = RemoteEntryMetadata {
            input_globs: Vec::new(),
            input_files: BTreeMap::from([(
                "main.py".to_string(),
                blake3::hash(b"print(1)").to_hex().to_string(),
            )]),
            record: dummy_record(),
        };
        client
            .register_entry(
                "somekey",
                &sha256,
                "foo-1.0.0-h0_0.conda",
                std::slice::from_ref(&git_hash),
                &metadata,
            )
            .await
            .unwrap();

        // The hash index matches the git hash.
        let matches = client
            .query("somekey", std::slice::from_ref(&git_hash))
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].artifact_sha256, hex::encode(sha256));

        // ... and does not match another hash.
        let matches = client
            .query("somekey", &["git:0000".to_string()])
            .await
            .unwrap();
        assert!(matches.is_empty());

        // The entry listing carries parseable metadata.
        let entries = client.entries("somekey").await.unwrap();
        assert_eq!(entries.len(), 1);
        let parsed = entries[0].parsed_metadata().unwrap();
        assert_eq!(parsed.input_files.len(), 1);

        // Downloading verifies the sha256 and yields the original bytes.
        let dest = tmp.path().join("downloaded.conda");
        client
            .download_artifact("somekey", &hex::encode(sha256), &dest)
            .await
            .unwrap();
        assert_eq!(fs_err::read(&dest).unwrap(), b"pretend conda");
    }

    fn dummy_record() -> RepoDataRecord {
        use std::str::FromStr;

        use rattler_conda_types::{PackageName, PackageRecord, VersionWithSource};

        let mut package_record = PackageRecord::new(
            PackageName::from_str("foo").unwrap(),
            VersionWithSource::from_str("1.0.0").unwrap(),
            "h0_0".to_string(),
        );
        package_record.subdir = "linux-64".to_string();
        RepoDataRecord {
            package_record,
            identifier: rattler_conda_types::package::DistArchiveIdentifier::try_from_filename(
                "foo-1.0.0-h0_0.conda",
            )
            .unwrap(),
            url: Url::parse("file:///foo-1.0.0-h0_0.conda").unwrap(),
            channel: None,
        }
    }

    #[test]
    fn endpoint_construction() {
        let client = RemoteBuildCacheClient::new(
            RemoteBuildCacheSettings {
                url: Url::parse("https://prefix.dev").unwrap(),
                owner: "me".to_string(),
                write: false,
            },
            LazyClient::from(reqwest::Client::new()),
        );
        assert_eq!(
            client.endpoint(&["entries", "abc123"]).unwrap().as_str(),
            "https://prefix.dev/api/v1/build-cache/me/entries/abc123",
        );
        assert_eq!(
            client.endpoint(&["query"]).unwrap().as_str(),
            "https://prefix.dev/api/v1/build-cache/me/query",
        );
    }
}
