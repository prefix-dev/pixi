//! Persisting channel-wide detector decisions and user-owned repository approvals.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};

use miette::{Context, IntoDiagnostic};
use pixi_consts::consts;
use rattler_conda_types::ChannelUrl;
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table, TableLike, value};

use crate::{DetectorDecision, VirtualPackageDetectorsConfig};

/// Stores a channel-wide `decision` in `path`, preserving unrelated settings,
/// comments, inline tables, and existing URL spellings.
pub fn write_detector_decision(
    path: &Path,
    origin: &ChannelUrl,
    decision: DetectorDecision,
) -> miette::Result<()> {
    let contents = match fs_err::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error)
                .into_diagnostic()
                .wrap_err_with(|| format!("failed to read '{}'", path.display()));
        }
    };
    let mut document = contents
        .parse::<DocumentMut>()
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to parse '{}'", path.display()))?;
    let mut table: &mut dyn TableLike = document.as_table_mut();
    for segment in ["virtual-package-detectors", "consent"] {
        let item = table.entry(segment).or_insert_with(|| {
            let mut new = Table::new();
            new.set_implicit(segment != "consent");
            Item::Table(new)
        });
        table = item
            .as_table_like_mut()
            .ok_or_else(|| miette::miette!("'{segment}' in '{}' is not a table", path.display()))?;
    }
    let canonical = canonical_channel(origin);
    let existing = table.iter().find_map(|(key, _)| {
        url::Url::parse(key)
            .ok()
            .map(ChannelUrl::from)
            .filter(|channel| channel == origin)
            .map(|_| key.to_string())
    });
    let key = existing.as_deref().unwrap_or(&canonical);
    if table.get(key).is_some_and(|item| item.as_str().is_none()) {
        miette::bail!(
            "consent for '{key}' must be a channel-wide scalar \"allow\" or \"deny\", not a per-detector table"
        );
    }
    let mut replacement = value(match decision {
        DetectorDecision::Allow => "allow",
        DetectorDecision::Deny => "deny",
    });
    if let Some(previous) = table.get(key).and_then(Item::as_value)
        && let Some(next) = replacement.as_value_mut()
    {
        *next.decor_mut() = previous.decor().clone();
    }
    table.insert(key, replacement);
    if let Some(parent) = path.parent() {
        fs_err::create_dir_all(parent)
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to create '{}'", parent.display()))?;
    }
    fs_err::write(path, document.to_string())
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to write '{}'", path.display()))
}

/// Returns the canonical repository's local configuration path, refusing
/// checkout-controlled links that could redirect a repository-scoped write.
pub fn repository_detector_config_path(project_root: &Path) -> miette::Result<PathBuf> {
    let repository = fs_err::canonicalize(project_root).into_diagnostic()?;
    let directory = repository.join(consts::PIXI_DIR);
    let path = directory.join(consts::CONFIG_FILE);
    for candidate in [&directory, &path] {
        match fs_err::symlink_metadata(candidate) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    miette::bail!(
                        "repository detector configuration '{}' must not be a symlink",
                        candidate.display()
                    );
                }
                if (candidate == &directory && !metadata.is_dir())
                    || (candidate == &path && !metadata.is_file())
                {
                    miette::bail!(
                        "repository detector configuration '{}' has an invalid file type",
                        candidate.display()
                    );
                }
                #[cfg(unix)]
                if metadata.is_file() && metadata.nlink() != 1 {
                    miette::bail!(
                        "repository detector configuration '{}' must not be hard-linked",
                        candidate.display()
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).into_diagnostic(),
        }
    }
    Ok(path)
}

/// Records an explicit repository-scoped decision in the local configuration
/// and in the user's approval registry outside the checkout.
pub fn write_repository_detector_decision(
    project_root: &Path,
    origin: &ChannelUrl,
    decision: DetectorDecision,
) -> miette::Result<()> {
    repository_detector_config_path(project_root)?;
    let store = ApprovalStore::for_repository(project_root)?;
    let lock = store.lock()?;
    let mut registry = store.read()?;
    registry.update(&store.repository, origin, Some(decision));
    let path = repository_detector_config_path(project_root)?;
    write_detector_decision(&path, origin, decision)?;
    store.write(&registry)?;
    drop(lock);
    Ok(())
}

/// Updates only the explicitly selected channel approvals after a local CLI
/// consent edit. `None` and `Deny` revoke approval, while `Allow` authorizes it.
pub fn update_repository_detector_approvals(
    project_root: &Path,
    decisions: impl IntoIterator<Item = (ChannelUrl, Option<DetectorDecision>)>,
) -> miette::Result<()> {
    let store = ApprovalStore::for_repository(project_root)?;
    let lock = store.lock()?;
    let mut registry = store.read()?;
    for (origin, decision) in decisions {
        registry.update(&store.repository, &origin, decision);
    }
    store.write(&registry)?;
    drop(lock);
    Ok(())
}

pub(crate) fn retain_approved_repository_consent(
    project_root: &Path,
    config: &mut VirtualPackageDetectorsConfig,
) {
    if config
        .consent
        .values()
        .all(|decision| *decision == DetectorDecision::Deny)
    {
        return;
    }
    let approved = ApprovalStore::for_repository(project_root)
        .and_then(|store| {
            let mut registry = store.read()?;
            Ok(registry
                .repositories
                .remove(&store.repository)
                .unwrap_or_default())
        })
        .unwrap_or_else(|error| {
            tracing::warn!("Ignoring repository detector approvals: {error}");
            BTreeSet::new()
        });
    config.consent.retain(|origin, decision| {
        *decision == DetectorDecision::Deny || approved.contains(&canonical_channel(origin))
    });
}

fn canonical_channel(origin: &ChannelUrl) -> String {
    origin.as_str().trim_end_matches('/').to_string()
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ApprovalRegistry {
    version: u32,
    repositories: BTreeMap<PathBuf, BTreeSet<String>>,
}

impl Default for ApprovalRegistry {
    fn default() -> Self {
        Self {
            version: 1,
            repositories: BTreeMap::new(),
        }
    }
}

impl ApprovalRegistry {
    fn update(
        &mut self,
        repository: &Path,
        origin: &ChannelUrl,
        decision: Option<DetectorDecision>,
    ) {
        if decision == Some(DetectorDecision::Allow) {
            self.repositories
                .entry(repository.to_path_buf())
                .or_default()
                .insert(canonical_channel(origin));
        } else if let Some(origins) = self.repositories.get_mut(repository) {
            origins.remove(&canonical_channel(origin));
            if origins.is_empty() {
                self.repositories.remove(repository);
            }
        }
    }
}

struct ApprovalStore {
    config_home: PathBuf,
    directory: PathBuf,
    repository: PathBuf,
}

impl ApprovalStore {
    fn for_repository(project_root: &Path) -> miette::Result<Self> {
        #[cfg(windows)]
        let override_home = std::env::var_os("APPDATA");
        #[cfg(not(windows))]
        let override_home = std::env::var_os("XDG_CONFIG_HOME");
        let config_home = override_home
            .map(PathBuf::from)
            .or_else(dirs::config_dir)
            .ok_or_else(|| miette::miette!("the user configuration directory is unknown"))?;
        Self::new(config_home, project_root)
    }

    fn new(config_home: PathBuf, project_root: &Path) -> miette::Result<Self> {
        if !config_home.is_absolute()
            || config_home
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            miette::bail!(
                "the detector approval directory must be an absolute user configuration path"
            );
        }
        let repository = fs_err::canonicalize(project_root).into_diagnostic()?;
        let directory = config_home.join(consts::CONFIG_DIR);
        let store = Self {
            config_home,
            directory,
            repository,
        };
        store.validate()?;
        Ok(store)
    }

    fn path(&self) -> PathBuf {
        self.directory.join("detector-approvals.json")
    }

    fn validate(&self) -> miette::Result<()> {
        // Resolve pre-existing ancestors (including OS aliases such as /var on
        // macOS) before testing containment. The registry itself never follows
        // symlinks, and no checkout-selected Pixi path is used here.
        let mut existing = self.directory.as_path();
        while !existing.exists() {
            existing = existing.parent().ok_or_else(|| {
                miette::miette!("the user configuration directory has no existing ancestor")
            })?;
        }
        let resolved = fs_err::canonicalize(existing).into_diagnostic()?;
        if resolved.starts_with(&self.repository) {
            miette::bail!("detector approvals must be stored outside the repository");
        }
        for ancestor in resolved.ancestors() {
            validate_owner(&fs_err::metadata(ancestor).into_diagnostic()?, false)?;
        }
        let registry_path = self.path();
        let lock_path = self.directory.join("detector-approvals.lock");
        for path in [
            &self.config_home,
            &self.directory,
            &registry_path,
            &lock_path,
        ] {
            match fs_err::symlink_metadata(path) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() {
                        miette::bail!(
                            "detector approval path '{}' must not be a symlink",
                            path.display()
                        );
                    }
                    validate_owner(&metadata, true)?;
                    if (path == &self.config_home || path == &self.directory) && !metadata.is_dir()
                    {
                        miette::bail!(
                            "detector approval directory '{}' is not a directory",
                            path.display()
                        );
                    }
                    if (path == &registry_path || path == &lock_path) && !metadata.is_file() {
                        miette::bail!(
                            "detector approval file '{}' is not a regular file",
                            path.display()
                        );
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).into_diagnostic(),
            }
        }
        Ok(())
    }

    fn lock(&self) -> miette::Result<File> {
        self.validate()?;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(&self.directory).into_diagnostic()?;
        self.validate()?;
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
        let lock = options
            .open(self.directory.join("detector-approvals.lock"))
            .into_diagnostic()?;
        lock.lock().into_diagnostic()?;
        self.validate()?;
        Ok(lock)
    }

    fn read(&self) -> miette::Result<ApprovalRegistry> {
        self.validate()?;
        let contents = match fs_err::read_to_string(self.path()) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ApprovalRegistry::default());
            }
            Err(error) => return Err(error).into_diagnostic(),
        };
        let registry: ApprovalRegistry = serde_json::from_str(&contents)
            .into_diagnostic()
            .wrap_err("failed to parse the repository detector approval registry")?;
        if registry.version != 1 {
            miette::bail!("unsupported repository detector approval registry version");
        }
        for (repository, channels) in &registry.repositories {
            if !repository.is_absolute()
                || repository
                    .components()
                    .any(|component| matches!(component, Component::ParentDir))
            {
                miette::bail!("invalid repository path in detector approval registry");
            }
            for channel in channels {
                let origin = url::Url::parse(channel).into_diagnostic()?;
                if origin.cannot_be_a_base()
                    || canonical_channel(&ChannelUrl::from(origin)) != *channel
                {
                    miette::bail!("non-canonical channel in detector approval registry");
                }
            }
        }
        Ok(registry)
    }

    fn write(&self, registry: &ApprovalRegistry) -> miette::Result<()> {
        self.validate()?;
        let mut file = tempfile::NamedTempFile::new_in(&self.directory).into_diagnostic()?;
        #[cfg(unix)]
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .into_diagnostic()?;
        let contents = serde_json::to_vec_pretty(registry).into_diagnostic()?;
        file.write_all(&contents).into_diagnostic()?;
        file.as_file().sync_all().into_diagnostic()?;
        file.persist(self.path()).into_diagnostic()?;
        Ok(())
    }
}

fn validate_owner(metadata: &std::fs::Metadata, require_user: bool) -> miette::Result<()> {
    #[cfg(unix)]
    {
        let uid = nix::unistd::getuid().as_raw();
        if (require_user && metadata.uid() != uid)
            || (!require_user && metadata.uid() != uid && metadata.uid() != 0)
            || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
            || (metadata.is_file() && metadata.nlink() != 1)
        {
            miette::bail!(
                "detector approval storage must be user-owned and not writable by other users"
            );
        }
    }
    #[cfg(not(unix))]
    let _ = (metadata, require_user);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn origin() -> ChannelUrl {
        ChannelUrl::from(url::Url::parse("https://prefix.dev/conda-forge/").unwrap())
    }

    #[cfg(unix)]
    #[test]
    fn repository_decisions_cannot_modify_linked_external_configs() {
        for link_kind in ["directory", "file", "hardlink"] {
            let directory = tempfile::tempdir().unwrap();
            let repository = directory.path().join("repository");
            let external = directory.path().join("external");
            fs_err::create_dir_all(&repository).unwrap();
            fs_err::create_dir_all(&external).unwrap();
            let target = external.join(consts::CONFIG_FILE);
            let original = "offline = true\n";
            fs_err::write(&target, original).unwrap();
            let local_directory = repository.join(consts::PIXI_DIR);
            let local = local_directory.join(consts::CONFIG_FILE);
            if link_kind == "directory" {
                std::os::unix::fs::symlink(&external, &local_directory).unwrap();
            } else {
                fs_err::create_dir(&local_directory).unwrap();
                if link_kind == "file" {
                    std::os::unix::fs::symlink(&target, &local).unwrap();
                } else {
                    fs_err::hard_link(&target, &local).unwrap();
                }
            }
            for decision in [DetectorDecision::Allow, DetectorDecision::Deny] {
                assert!(
                    write_repository_detector_decision(&repository, &origin(), decision).is_err(),
                    "{link_kind}"
                );
                assert_eq!(fs_err::read_to_string(&target).unwrap(), original);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn repository_root_symlinks_share_canonical_approval_without_redirecting_config() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repository");
        let alias = directory.path().join("alias");
        let config_home = directory.path().join("user-config");
        fs_err::create_dir_all(&repository).unwrap();
        std::os::unix::fs::symlink(&repository, &alias).unwrap();
        temp_env::with_vars(
            [
                ("XDG_CONFIG_HOME", Some(config_home.as_os_str())),
                ("APPDATA", Some(config_home.as_os_str())),
            ],
            || {
                write_repository_detector_decision(&alias, &origin(), DetectorDecision::Allow)
                    .unwrap();
                for root in [&repository, &alias] {
                    assert_eq!(
                        Config::load_with(root, &crate::GlobalConfigSource::None)
                            .virtual_package_detectors
                            .consent(&origin()),
                        Some(DetectorDecision::Allow)
                    );
                }
                write_repository_detector_decision(&repository, &origin(), DetectorDecision::Deny)
                    .unwrap();
                write_detector_decision(
                    &alias.join(consts::PIXI_DIR).join(consts::CONFIG_FILE),
                    &origin(),
                    DetectorDecision::Allow,
                )
                .unwrap();
                assert_eq!(
                    Config::load_with(&alias, &crate::GlobalConfigSource::None)
                        .virtual_package_detectors
                        .consent(&origin()),
                    None
                );
            },
        );
    }

    #[test]
    fn decisions_preserve_other_settings_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs_err::write(&path, "# keep me\ndefault-channels = [\"conda-forge\"]\n\n[virtual-package-detectors]\ntimeout-seconds = 10\n").unwrap();
        write_detector_decision(&path, &origin(), DetectorDecision::Allow).unwrap();
        let config = Config::from_path(&path).unwrap();
        assert_eq!(
            config.virtual_package_detectors.consent(&origin()),
            Some(DetectorDecision::Allow)
        );
        assert_eq!(config.virtual_package_detectors.timeout_seconds, Some(10));
        assert_eq!(
            config.default_channels,
            vec!["conda-forge".parse().unwrap()]
        );
        assert!(
            fs_err::read_to_string(&path)
                .unwrap()
                .starts_with("# keep me\n")
        );
        write_detector_decision(&path, &origin(), DetectorDecision::Deny).unwrap();
        assert_eq!(
            Config::from_path(&path)
                .unwrap()
                .virtual_package_detectors
                .consent(&origin()),
            Some(DetectorDecision::Deny)
        );
    }

    #[test]
    fn trailing_slash_and_inline_table_are_reused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs_err::write(&path, "[virtual-package-detectors]\nconsent = { \"https://prefix.dev/conda-forge/\" = \"deny\" } # keep comment\n").unwrap();
        write_detector_decision(&path, &origin(), DetectorDecision::Allow).unwrap();
        let config = Config::from_path(&path).unwrap();
        assert_eq!(
            config.virtual_package_detectors.consent(&origin()),
            Some(DetectorDecision::Allow)
        );
        assert_eq!(config.virtual_package_detectors.consent.len(), 1);
        let written = fs_err::read_to_string(&path).unwrap();
        assert!(written.contains("# keep comment"));
        assert!(written.contains("consent = {"));
    }

    #[test]
    fn approvals_are_repository_and_channel_bound_and_revocable() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        fs_err::create_dir_all(&first).unwrap();
        fs_err::create_dir_all(&second).unwrap();
        let store = ApprovalStore::new(directory.path().join("config"), &first).unwrap();
        let other = ApprovalStore::new(directory.path().join("config"), &second).unwrap();
        let another_channel =
            ChannelUrl::from(url::Url::parse("https://prefix.dev/another").unwrap());
        let _lock = store.lock().unwrap();
        let mut registry = store.read().unwrap();
        registry.update(&store.repository, &origin(), Some(DetectorDecision::Allow));
        store.write(&registry).unwrap();
        assert_eq!(
            store.read().unwrap().repositories[&store.repository],
            BTreeSet::from([canonical_channel(&origin())])
        );
        assert!(
            !other
                .read()
                .unwrap()
                .repositories
                .contains_key(&other.repository)
        );
        assert!(
            !store.read().unwrap().repositories[&store.repository]
                .contains(&canonical_channel(&another_channel))
        );
        registry.update(&store.repository, &origin(), Some(DetectorDecision::Deny));
        store.write(&registry).unwrap();
        assert!(
            !store
                .read()
                .unwrap()
                .repositories
                .contains_key(&store.repository)
        );
        registry.update(&store.repository, &origin(), Some(DetectorDecision::Allow));
        registry.update(&store.repository, &origin(), None);
        store.write(&registry).unwrap();
        assert!(
            !store
                .read()
                .unwrap()
                .repositories
                .contains_key(&store.repository)
        );
    }

    #[test]
    fn checkout_cannot_host_or_redirect_the_approval_registry() {
        let directory = tempfile::tempdir().unwrap();
        assert!(ApprovalStore::new(directory.path().join("config"), directory.path()).is_err());
        let repository = directory.path().join("repository");
        let config_home = directory.path().join("config");
        fs_err::create_dir_all(&repository).unwrap();
        fs_err::create_dir_all(&config_home).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&repository, config_home.join(consts::CONFIG_DIR)).unwrap();
            assert!(ApprovalStore::new(config_home, &repository).is_err());
        }
    }

    #[test]
    fn malformed_registry_is_not_replaced_or_used() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repository");
        fs_err::create_dir_all(&repository).unwrap();
        let store = ApprovalStore::new(directory.path().join("config"), &repository).unwrap();
        let _lock = store.lock().unwrap();
        fs_err::write(store.path(), "{broken}").unwrap();
        assert!(store.read().is_err());
        assert_eq!(fs_err::read_to_string(store.path()).unwrap(), "{broken}");
    }

    #[test]
    fn local_allows_require_approval_and_approved_local_decisions_take_precedence() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repository");
        let config_home = directory.path().join("user-config");
        let shared = directory.path().join("shared.toml");
        fs_err::create_dir_all(&repository).unwrap();
        let local = repository.join(consts::PIXI_DIR).join(consts::CONFIG_FILE);
        temp_env::with_vars(
            [
                ("XDG_CONFIG_HOME", Some(config_home.as_os_str())),
                ("APPDATA", Some(config_home.as_os_str())),
            ],
            || {
                write_detector_decision(&shared, &origin(), DetectorDecision::Deny).unwrap();
                write_detector_decision(&local, &origin(), DetectorDecision::Allow).unwrap();
                let source = crate::GlobalConfigSource::File(shared.clone());
                assert_eq!(
                    Config::load_with(&repository, &source)
                        .virtual_package_detectors
                        .consent(&origin()),
                    Some(DetectorDecision::Deny)
                );
                write_repository_detector_decision(&repository, &origin(), DetectorDecision::Allow)
                    .unwrap();
                assert_eq!(
                    Config::load_with(&repository, &source)
                        .virtual_package_detectors
                        .consent(&origin()),
                    Some(DetectorDecision::Allow)
                );
                write_detector_decision(&shared, &origin(), DetectorDecision::Allow).unwrap();
                write_repository_detector_decision(&repository, &origin(), DetectorDecision::Deny)
                    .unwrap();
                assert_eq!(
                    Config::load_with(&repository, &source)
                        .virtual_package_detectors
                        .consent(&origin()),
                    Some(DetectorDecision::Deny)
                );
                write_detector_decision(&shared, &origin(), DetectorDecision::Deny).unwrap();
                write_detector_decision(&local, &origin(), DetectorDecision::Allow).unwrap();
                assert_eq!(
                    Config::load_with(&repository, &source)
                        .virtual_package_detectors
                        .consent(&origin()),
                    Some(DetectorDecision::Deny)
                );
                write_repository_detector_decision(&repository, &origin(), DetectorDecision::Allow)
                    .unwrap();
                update_repository_detector_approvals(&repository, [(origin(), None)]).unwrap();
                assert_eq!(
                    Config::load_with(&repository, &source)
                        .virtual_package_detectors
                        .consent(&origin()),
                    Some(DetectorDecision::Deny)
                );
            },
        );
    }

    #[test]
    fn copied_local_config_and_unusable_registries_do_not_grant_execution() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        let config_home = directory.path().join("user-config");
        fs_err::create_dir_all(&first).unwrap();
        fs_err::create_dir_all(&second).unwrap();
        temp_env::with_vars(
            [
                ("XDG_CONFIG_HOME", Some(config_home.as_os_str())),
                ("APPDATA", Some(config_home.as_os_str())),
            ],
            || {
                let source = crate::GlobalConfigSource::None;
                write_repository_detector_decision(&first, &origin(), DetectorDecision::Allow)
                    .unwrap();
                write_detector_decision(
                    &second.join(consts::PIXI_DIR).join(consts::CONFIG_FILE),
                    &origin(),
                    DetectorDecision::Allow,
                )
                .unwrap();
                assert_eq!(
                    Config::load_with(&first, &source)
                        .virtual_package_detectors
                        .consent(&origin()),
                    Some(DetectorDecision::Allow)
                );
                assert_eq!(
                    Config::load_with(&second, &source)
                        .virtual_package_detectors
                        .consent(&origin()),
                    None
                );
                let other = ChannelUrl::from(url::Url::parse("https://prefix.dev/other").unwrap());
                let local = first.join(consts::PIXI_DIR).join(consts::CONFIG_FILE);
                write_detector_decision(&local, &other, DetectorDecision::Allow).unwrap();
                assert_eq!(
                    Config::load_with(&first, &source)
                        .virtual_package_detectors
                        .consent(&other),
                    None
                );
                let store = ApprovalStore::for_repository(&first).unwrap();
                fs_err::write(store.path(), "invalid").unwrap();
                assert_eq!(
                    Config::load_with(&first, &source)
                        .virtual_package_detectors
                        .consent(&origin()),
                    None
                );
                fs_err::remove_file(store.path()).unwrap();
                fs_err::create_dir(store.path()).unwrap();
                assert_eq!(
                    Config::load_with(&first, &source)
                        .virtual_package_detectors
                        .consent(&origin()),
                    None
                );
                write_detector_decision(&local, &origin(), DetectorDecision::Deny).unwrap();
                assert_eq!(
                    Config::load_with(&first, &source)
                        .virtual_package_detectors
                        .consent(&origin()),
                    Some(DetectorDecision::Deny)
                );
            },
        );
    }
}
