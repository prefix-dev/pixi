/// This module contains code facilitating man page support for `pixi global`
use std::path::{Path, PathBuf};

use indexmap::IndexSet;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_config::pixi_home;

use super::Mapping;
use super::StateChange;
use fs_err::tokio as tokio_fs;

/// Global man pages directory, default to `$HOME/.pixi/share/man`
#[derive(Debug, Clone)]
pub struct ManDir(PathBuf);

impl ManDir {
    /// Create the global man pages directory from environment variables
    pub async fn from_env() -> miette::Result<Self> {
        let man_dir = pixi_home()
            .map(|path| path.join("share").join("man"))
            .ok_or(miette::miette!(
                "Couldn't determine global man pages directory"
            ))?;
        tokio_fs::create_dir_all(&man_dir).await.into_diagnostic()?;

        // Create standard man page sections
        for section in ["man1", "man3", "man5", "man8"] {
            tokio_fs::create_dir_all(man_dir.join(section))
                .await
                .into_diagnostic()?;
        }

        Ok(Self(man_dir))
    }

    /// Returns the path to the man pages directory
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Prune old man pages
    pub fn prune_old_man_pages(&self) -> miette::Result<()> {
        for section in ["man1", "man3", "man5", "man8"] {
            let section_dir = self.section_path(section);
            if !section_dir.is_dir() {
                continue;
            }

            for entry in fs_err::read_dir(&section_dir).into_diagnostic()? {
                let path = entry.into_diagnostic()?.path();

                if (path.is_symlink() && fs_err::read_link(&path).is_err())
                    || (!path.is_symlink() && path.is_file())
                {
                    // Remove broken symlink or non-symlink files
                    fs_err::remove_file(&path).into_diagnostic()?;
                }
            }
        }

        Ok(())
    }

    pub fn section_path(&self, section: &str) -> PathBuf {
        self.path().join(section)
    }
}

#[derive(Debug, Clone)]
pub struct ManPage {
    name: String,
    source: PathBuf,
    destination: PathBuf,
    #[allow(dead_code)] // Used in tests but may be useful for future features
    section: String,
}

impl ManPage {
    pub fn new(name: String, source: PathBuf, destination: PathBuf, section: String) -> Self {
        Self {
            name,
            source,
            destination,
            section,
        }
    }

    /// Install the man page
    pub async fn install(&self) -> miette::Result<Option<StateChange>> {
        tracing::debug!("Requested to install man page {}.", self.source.display());

        // Ensure the parent directory of the destination exists
        if let Some(parent) = self.destination.parent() {
            tokio_fs::create_dir_all(parent).await.into_diagnostic()?;
        }

        // Attempt to create the symlink
        tokio_fs::symlink(&self.source, &self.destination)
            .await
            .into_diagnostic()?;

        Ok(Some(StateChange::AddedManPage(self.name.clone())))
    }

    /// Remove the man page
    pub async fn remove(&self) -> miette::Result<StateChange> {
        tokio_fs::remove_file(&self.destination)
            .await
            .into_diagnostic()?;

        Ok(StateChange::RemovedManPage(self.name.clone()))
    }
}

/// Generates a list of man pages for a given executable name.
///
/// This function checks for the existence of man pages for the specified command
/// in the prefix_root directory. It looks in the standard man page sections (man1, man8, man3, man5)
/// and returns the first match found, prioritizing user commands (man1) and system commands (man8).
pub fn contained_man_pages(
    prefix_root: &Path,
    name: &str,
    man_dir: &ManDir,
) -> miette::Result<Vec<ManPage>> {
    let mut man_pages = Vec::new();

    // Priority order: man1 (user commands), man8 (system admin), man3 (library), man5 (config)
    let sections = [("man1", "1"), ("man8", "8"), ("man3", "3"), ("man5", "5")];

    for (section_dir, section_num) in sections {
        let man_page_name = format!("{name}.{section_num}");
        let man_path = prefix_root
            .join("share")
            .join("man")
            .join(section_dir)
            .join(&man_page_name);

        if man_path.exists() {
            let destination = man_dir.section_path(section_dir).join(&man_page_name);

            man_pages.push(ManPage::new(
                name.to_string(),
                man_path,
                destination,
                section_num.to_string(),
            ));

            // Only take the first man page found for each command
            break;
        }
    }

    Ok(man_pages)
}

/// Synchronizes the man pages for the given executable names.
///
/// This function determines which man pages need to be removed or added
/// based on the provided `exposed_mappings` and `executable_names`. It compares the
/// current state of the man pages in the `man_dir` with the expected
/// state derived from the `exposed_mappings`.
pub(crate) async fn man_pages_sync_status(
    exposed_mappings: IndexSet<Mapping>,
    executable_names: Vec<String>,
    prefix_root: &Path,
    man_dir: &ManDir,
) -> miette::Result<(Vec<ManPage>, Vec<ManPage>)> {
    let mut man_pages_to_add = Vec::new();
    let mut man_pages_to_remove = Vec::new();

    let exposed_names = exposed_mappings
        .into_iter()
        .filter(|mapping| mapping.exposed_name().to_string() == mapping.executable_name())
        .map(|name| name.executable_name().to_string())
        .collect_vec();

    for name in executable_names.into_iter().unique() {
        let man_pages = contained_man_pages(prefix_root, &name, man_dir)?;

        if man_pages.is_empty() {
            continue;
        }

        if exposed_names.contains(&name) {
            for man_page in man_pages {
                if !man_page.destination.is_symlink() {
                    man_pages_to_add.push(man_page);
                }
            }
        } else {
            for man_page in man_pages {
                if man_page.destination.is_symlink() {
                    if let Ok(target) = tokio_fs::read_link(&man_page.destination).await {
                        if target == man_page.source {
                            man_pages_to_remove.push(man_page);
                        }
                    }
                }
            }
        }
    }

    Ok((man_pages_to_remove, man_pages_to_add))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err as fs;
    use tempfile::tempdir;

    #[test]
    fn test_man_dir_path_creation() {
        let temp_dir = tempdir().unwrap();
        let man_dir_path = temp_dir.path().join("share").join("man");

        // Test ManDir creation directly without async
        let man_dir = ManDir(man_dir_path.clone());
        assert_eq!(man_dir.path(), &man_dir_path);

        // Test section path creation
        let man1_path = man_dir.section_path("man1");
        assert_eq!(man1_path, man_dir_path.join("man1"));
    }

    #[test]
    fn test_contained_man_pages_finds_man1() {
        let temp_dir = tempdir().unwrap();
        let prefix_root = temp_dir.path();

        // Create man page structure
        let man1_dir = prefix_root.join("share").join("man").join("man1");
        fs::create_dir_all(&man1_dir).unwrap();

        // Create a man page
        let man_page = man1_dir.join("test.1");
        fs::write(&man_page, "test man page content").unwrap();

        // Create mock ManDir
        let man_dir_path = temp_dir.path().join("global_man");
        fs::create_dir_all(&man_dir_path).unwrap();
        let man_dir = ManDir(man_dir_path);

        // Test the function
        let result = contained_man_pages(prefix_root, "test", &man_dir).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "test");
        assert_eq!(result[0].section, "1");
        assert!(result[0].source.ends_with("test.1"));
    }

    #[test]
    fn test_contained_man_pages_priority_order() {
        let temp_dir = tempdir().unwrap();
        let prefix_root = temp_dir.path();

        // Create man page structure with multiple sections
        for section in ["man1", "man3", "man5"] {
            let section_dir = prefix_root.join("share").join("man").join(section);
            fs::create_dir_all(&section_dir).unwrap();

            let section_num = section.strip_prefix("man").unwrap();
            let man_page = section_dir.join(format!("test.{}", section_num));
            fs::write(&man_page, format!("test man page section {}", section_num)).unwrap();
        }

        // Create mock ManDir
        let man_dir_path = temp_dir.path().join("global_man");
        fs::create_dir_all(&man_dir_path).unwrap();
        let man_dir = ManDir(man_dir_path);

        // Test the function - should return only man1 (highest priority)
        let result = contained_man_pages(prefix_root, "test", &man_dir).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].section, "1");
    }

    #[test]
    fn test_contained_man_pages_no_man_page() {
        let temp_dir = tempdir().unwrap();
        let prefix_root = temp_dir.path();

        // Create man directory but no man pages
        let man1_dir = prefix_root.join("share").join("man").join("man1");
        fs::create_dir_all(&man1_dir).unwrap();

        // Create mock ManDir
        let man_dir_path = temp_dir.path().join("global_man");
        fs::create_dir_all(&man_dir_path).unwrap();
        let man_dir = ManDir(man_dir_path);

        // Test the function
        let result = contained_man_pages(prefix_root, "nonexistent", &man_dir).unwrap();

        assert_eq!(result.len(), 0);
    }
}
