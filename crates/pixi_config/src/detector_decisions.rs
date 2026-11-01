//! Persisting channel-wide detector decisions.

use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use miette::Diagnostic;
use pixi_consts::consts;
use rattler_conda_types::ChannelUrl;
use thiserror::Error;
use toml_edit::{DocumentMut, Item, Table, TableLike, value};

use crate::DetectorDecision;

/// The registration channel's base URL without embedded credentials or tokens.
pub fn consent_origin(url: &ChannelUrl) -> ChannelUrl {
    ChannelUrl::from(rattler_redaction::strip_url_for_serialization(url.as_ref()))
}

/// Failure to validate or persist a channel-wide detector decision.
#[derive(Debug, Error, Diagnostic)]
pub enum DetectorConfigError {
    #[error("failed to read '{}'", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse '{}'", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml_edit::TomlError,
    },
    #[error("'{segment}' in '{}' is not a table", path.display())]
    NotATable {
        path: PathBuf,
        segment: &'static str,
    },
    #[error(
        "consent for '{key}' must be a channel-wide scalar \"allow\" or \"deny\", not a per-detector table"
    )]
    InvalidConsent { key: String },
    #[error("failed to create '{}'", path.display())]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write '{}'", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to resolve repository '{}'", path.display())]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to inspect repository detector configuration '{}'", path.display())]
    Metadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("repository detector configuration '{}' must not be a symlink", .0.display())]
    Symlink(PathBuf),
    #[error("repository detector configuration '{}' has an invalid file type", .0.display())]
    InvalidFileType(PathBuf),
    #[cfg(unix)]
    #[error("repository detector configuration '{}' must not be hard-linked", .0.display())]
    HardLink(PathBuf),
}

/// Stores a channel-wide `decision` in `path`, preserving unrelated settings,
/// comments, inline tables, and existing URL spellings.
pub fn write_detector_decision(
    path: &Path,
    origin: &ChannelUrl,
    decision: DetectorDecision,
) -> Result<(), DetectorConfigError> {
    let contents = match fs_err::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(DetectorConfigError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    let mut document =
        contents
            .parse::<DocumentMut>()
            .map_err(|source| DetectorConfigError::Parse {
                path: path.to_owned(),
                source,
            })?;
    let mut table: &mut dyn TableLike = document.as_table_mut();
    for segment in ["virtual-package-detectors", "consent"] {
        let item = table.entry(segment).or_insert_with(|| {
            let mut new = Table::new();
            new.set_implicit(segment != "consent");
            Item::Table(new)
        });
        table = item
            .as_table_like_mut()
            .ok_or_else(|| DetectorConfigError::NotATable {
                path: path.to_owned(),
                segment,
            })?;
    }
    let origin = consent_origin(origin);
    let canonical = canonical_channel(&origin);
    let existing: Vec<String> = table
        .iter()
        .filter_map(|(key, _)| {
            url::Url::parse(key)
                .ok()
                .map(ChannelUrl::from)
                .filter(|channel| consent_origin(channel) == origin)
                .map(|_| key.to_string())
        })
        .collect();
    let key = existing
        .iter()
        .find(|key| {
            url::Url::parse(key)
                .ok()
                .map(ChannelUrl::from)
                .is_some_and(|channel| channel == origin)
        })
        .map(String::as_str)
        .unwrap_or(&canonical);
    if table.get(key).is_some_and(|item| item.as_str().is_none()) {
        return Err(DetectorConfigError::InvalidConsent {
            key: key.to_owned(),
        });
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
    for spelling in &existing {
        if spelling != key {
            table.remove(spelling);
        }
    }
    table.insert(key, replacement);
    if let Some(parent) = path.parent() {
        fs_err::create_dir_all(parent).map_err(|source| DetectorConfigError::CreateDirectory {
            path: parent.to_owned(),
            source,
        })?;
    }
    fs_err::write(path, document.to_string()).map_err(|source| DetectorConfigError::Write {
        path: path.to_owned(),
        source,
    })
}

/// Returns the canonical repository's local configuration path, refusing
/// checkout-controlled links that could redirect a repository-scoped write.
pub fn repository_detector_config_path(
    project_root: &Path,
) -> Result<PathBuf, DetectorConfigError> {
    let repository =
        fs_err::canonicalize(project_root).map_err(|source| DetectorConfigError::Canonicalize {
            path: project_root.to_owned(),
            source,
        })?;
    let directory = repository.join(consts::PIXI_DIR);
    let path = directory.join(consts::CONFIG_FILE);
    for candidate in [&directory, &path] {
        match fs_err::symlink_metadata(candidate) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(DetectorConfigError::Symlink(candidate.to_owned()));
                }
                if (candidate == &directory && !metadata.is_dir())
                    || (candidate == &path && !metadata.is_file())
                {
                    return Err(DetectorConfigError::InvalidFileType(candidate.to_owned()));
                }
                #[cfg(unix)]
                if metadata.is_file() && metadata.nlink() != 1 {
                    return Err(DetectorConfigError::HardLink(candidate.to_owned()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(DetectorConfigError::Metadata {
                    path: candidate.to_owned(),
                    source,
                });
            }
        }
    }
    Ok(path)
}

/// Stores a shareable repository-scoped decision in `.pixi/config.toml`.
pub fn write_repository_detector_decision(
    project_root: &Path,
    origin: &ChannelUrl,
    decision: DetectorDecision,
) -> Result<(), DetectorConfigError> {
    let path = repository_detector_config_path(project_root)?;
    write_detector_decision(&path, origin, decision)
}

fn canonical_channel(origin: &ChannelUrl) -> String {
    origin.as_str().trim_end_matches('/').to_string()
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
    fn repository_root_symlinks_write_to_the_repository_config() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repository");
        let alias = directory.path().join("alias");
        fs_err::create_dir_all(&repository).unwrap();
        std::os::unix::fs::symlink(&repository, &alias).unwrap();
        write_repository_detector_decision(&alias, &origin(), DetectorDecision::Allow).unwrap();
        assert_eq!(
            Config::load_with(&repository, &crate::GlobalConfigSource::None)
                .virtual_package_detectors
                .consent(&origin()),
            Some(DetectorDecision::Allow)
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
    fn decisions_replace_all_equivalent_origin_spellings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs_err::write(
            &path,
            "[virtual-package-detectors.consent]\n\
             \"https://prefix.dev/conda-forge\" = \"allow\"\n\
             \"https://prefix.dev/conda-forge/\" = \"allow\"\n\
             \"https://user:secret@prefix.dev/t/token/conda-forge\" = \"allow\"\n",
        )
        .unwrap();
        write_detector_decision(&path, &origin(), DetectorDecision::Deny).unwrap();
        assert_eq!(
            Config::from_path(&path)
                .unwrap()
                .virtual_package_detectors
                .consent(&origin()),
            Some(DetectorDecision::Deny)
        );
        assert!(!fs_err::read_to_string(&path).unwrap().contains("secret"));
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
    fn a_deny_wins_over_an_allow_from_another_layer() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repository");
        let shared = directory.path().join("shared.toml");
        let local = repository.join(consts::PIXI_DIR).join(consts::CONFIG_FILE);
        let source = crate::GlobalConfigSource::File(shared.clone());
        for (global, repository_decision) in [
            (DetectorDecision::Deny, DetectorDecision::Allow),
            (DetectorDecision::Allow, DetectorDecision::Deny),
        ] {
            write_detector_decision(&shared, &origin(), global).unwrap();
            write_detector_decision(&local, &origin(), repository_decision).unwrap();
            assert_eq!(
                Config::load_with(&repository, &source)
                    .virtual_package_detectors
                    .consent(&origin()),
                Some(DetectorDecision::Deny)
            );
        }
    }

    #[test]
    fn repository_decisions_are_shared_with_copied_configs() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        let first_config = first.join(consts::PIXI_DIR).join(consts::CONFIG_FILE);
        let second_config = second.join(consts::PIXI_DIR).join(consts::CONFIG_FILE);
        write_detector_decision(&first_config, &origin(), DetectorDecision::Allow).unwrap();
        fs_err::create_dir_all(second_config.parent().unwrap()).unwrap();
        fs_err::copy(&first_config, &second_config).unwrap();
        for repository in [&first, &second] {
            assert_eq!(
                Config::load_with(repository, &crate::GlobalConfigSource::None)
                    .virtual_package_detectors
                    .consent(&origin()),
                Some(DetectorDecision::Allow)
            );
        }
    }
}
