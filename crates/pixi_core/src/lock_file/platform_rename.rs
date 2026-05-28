//! Align lockfile platform names with the workspace manifest at load time.
//!
//! A pixi.toml entry can be renamed by hand (`linux-64-cuda` → `gpu-linux`)
//! without forcing the lockfile to be regenerated: the platform's identity --
//! its conda subdir plus the *customised* virtual packages declared by the
//! user -- has not changed, only the workspace-scoped label. Rather than make
//! every lockfile consumer cope with two names for the same target, we rewrite
//! the locked entries to use the manifest's current names as soon as the
//! lockfile is loaded. After this pass, name-based queries against the
//! returned [`LockFile`] read the same identifiers the workspace uses.
//!
//! Only safe renames are applied: each manifest platform must match exactly
//! one locked entry by identity, and the manifest name must not already be in
//! use by another platform in the lockfile. Mismatched or ambiguous entries
//! pass through unchanged so the satisfiability layer can still flag them.
use std::collections::HashMap;

use pixi_manifest::{PixiPlatform, WorkspaceManifest, platform};
use rattler_conda_types::GenericVirtualPackage;
use rattler_lock::{LockFile, PlatformData, PlatformName};

/// Rewrite locked platform names so they match the manifest's current names
/// where the identity matches. Returns the original lockfile unchanged when
/// no renames apply.
pub(crate) fn align_platform_names(lock_file: LockFile, manifest: &WorkspaceManifest) -> LockFile {
    let renames = compute_renames(&lock_file, manifest);
    if renames.is_empty() {
        return lock_file;
    }
    match rebuild_with_renames(&lock_file, &renames) {
        Ok(rebuilt) => rebuilt,
        // The rebuild only fails if the rattler builder rejects an input we
        // just read out of a valid lockfile. That would be a rattler bug; we
        // log and fall back to the unmodified lockfile so the user still gets
        // a working (though stale-named) result.
        Err(err) => {
            tracing::warn!(
                "failed to rewrite lockfile platform names against the manifest: {err}; \
                 continuing with the lockfile's original names",
            );
            lock_file
        }
    }
}

/// Build a `locked_name -> workspace_name` map for every locked platform that
/// unambiguously matches exactly one manifest platform by identity. Manifest
/// names that collide with another locked entry's name are skipped to keep the
/// rebuilt lockfile free of duplicates.
fn compute_renames(lock_file: &LockFile, manifest: &WorkspaceManifest) -> HashMap<String, String> {
    let workspace_platforms: Vec<&PixiPlatform> = manifest.workspace.platforms.iter().collect();
    let locked_names: std::collections::HashSet<String> = lock_file
        .platforms()
        .map(|p| p.name().to_string())
        .collect();
    let mut renames: HashMap<String, String> = HashMap::new();

    for locked in lock_file.platforms() {
        let locked_name = locked.name().to_string();
        let locked_identity = locked_customisations(&locked);

        // Already named what some manifest platform asks for? Leave it alone:
        // a different manifest entry might match by identity, but renaming it
        // onto the same string would create a duplicate.
        let already_matches_a_manifest_name = workspace_platforms
            .iter()
            .any(|wp| wp.name().as_str() == locked_name);

        let mut matching = workspace_platforms.iter().filter(|wp| {
            wp.subdir() == locked.subdir() && workspace_customisations(wp) == locked_identity
        });
        let first = matching.next();
        let second = matching.next();
        let Some(target) = first else {
            continue;
        };
        if second.is_some() {
            // Ambiguous match: two manifest platforms have the same identity
            // (rare, but possible if a user manually constructs them). Don't
            // pick one arbitrarily.
            continue;
        }
        let target_name = target.name().as_str();
        if target_name == locked_name {
            continue;
        }
        // The target name is already taken by *another* locked entry: a
        // rename would clobber it. Skip rather than silently drop a row.
        if locked_names.contains(target_name) {
            continue;
        }
        if already_matches_a_manifest_name {
            // Another manifest entry already references this locked row by
            // its current name; renaming it would orphan that reference.
            continue;
        }
        renames.insert(locked_name, target_name.to_string());
    }

    renames
}

/// Identity-matching VPs for a manifest platform: drop the materialised
/// subdir defaults so only user-set customisations participate in the match.
fn workspace_customisations(platform: &PixiPlatform) -> Vec<GenericVirtualPackage> {
    let subdir = platform.subdir();
    let mut customised: Vec<GenericVirtualPackage> = platform
        .declared_virtual_packages()
        .iter()
        .filter(|gvp| !platform::is_subdir_default(gvp, subdir))
        .cloned()
        .collect();
    customised.sort_by(|a, b| a.name.as_normalized().cmp(b.name.as_normalized()));
    customised
}

/// Identity-matching VPs for a locked platform: parse the lockfile's
/// `__name=version[=build]` strings back into [`GenericVirtualPackage`]s, drop
/// the entries that match the subdir's defaults, and sort by name. Strings
/// that don't parse are dropped -- the workspace side can't have a
/// corresponding entry anyway.
fn locked_customisations(locked: &rattler_lock::Platform<'_>) -> Vec<GenericVirtualPackage> {
    let subdir = locked.subdir();
    let mut customised: Vec<GenericVirtualPackage> = locked
        .virtual_packages()
        .iter()
        .filter_map(|raw| platform::parse_locked_virtual_package(raw))
        .filter(|gvp| !platform::is_subdir_default(gvp, subdir))
        .collect();
    customised.sort_by(|a, b| a.name.as_normalized().cmp(b.name.as_normalized()));
    customised
}

/// Rebuild the lockfile with renamed platform entries via the
/// [`rattler_lock::LockFileBuilder`]. Channels, indexes, solve options, and
/// packages are copied across verbatim; only the [`PlatformData::name`] of
/// each renamed entry changes.
fn rebuild_with_renames(
    lock_file: &LockFile,
    renames: &HashMap<String, String>,
) -> Result<LockFile, rattler_lock::ParseCondaLockError> {
    let mut builder = LockFile::builder();
    let platforms: Vec<PlatformData> = lock_file
        .platforms()
        .map(|p| {
            let current = p.name().to_string();
            let new_name = renames.get(&current).cloned().unwrap_or(current);
            PlatformData {
                name: PlatformName::try_from(new_name).expect(
                    "platform name validated by `PixiPlatformName` already passes \
                     rattler_lock's looser PlatformName grammar",
                ),
                subdir: p.subdir(),
                virtual_packages: p.virtual_packages().to_vec(),
            }
        })
        .collect();
    builder = builder.with_platforms(platforms)?;

    for (env_name, env) in lock_file.environments() {
        builder.set_channels(env_name, env.channels().iter().cloned());
        if let Some(indexes) = env.pypi_indexes() {
            builder.set_pypi_indexes(env_name, indexes.clone());
        }
        builder.set_options(env_name, env.solve_options().clone());
        for (platform, packages) in env.packages_by_platform() {
            let raw_name = platform.name().to_string();
            let resolved = renames
                .get(&raw_name)
                .map(String::as_str)
                .unwrap_or(raw_name.as_str());
            for package in packages {
                builder.add_package(env_name, resolved, package.clone())?;
            }
        }
    }

    Ok(builder.finish())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pixi_manifest::WorkspaceManifest;
    use rattler_conda_types::Platform;
    use rattler_lock::{LockFile, PlatformData, PlatformName};

    use super::align_platform_names;

    fn manifest(source: &str) -> WorkspaceManifest {
        WorkspaceManifest::from_toml_str_with_base_dir(source, Path::new("")).unwrap()
    }

    /// Build a lockfile with one platform `name` (subdir `subdir`,
    /// virtual packages `vps`) and one empty default environment that has
    /// been "solved" for it. Enough to exercise the rename pass.
    fn lockfile_with(name: &str, subdir: Platform, vps: Vec<String>) -> LockFile {
        let builder = LockFile::builder()
            .with_platforms(vec![PlatformData {
                name: PlatformName::try_from(name).unwrap(),
                subdir,
                virtual_packages: vps,
            }])
            .unwrap();
        let mut builder = builder;
        builder.set_channels("default", Vec::<rattler_lock::Channel>::new());
        builder.set_options("default", rattler_lock::SolveOptions::default());
        builder.finish()
    }

    /// A workspace that renamed `linux-64-cuda` to `gpu-linux` (same
    /// identity: linux-64 + `__cuda=12.0`) should pick up the old locked
    /// row under the new name without any user re-solve.
    #[test]
    fn rename_matches_renamed_workspace_platform() {
        let manifest = manifest(
            r#"
            [workspace]
            name = "demo"
            channels = []
            platforms = [{ name = "gpu-linux", platform = "linux-64", cuda = "12.0" }]
            "#,
        );
        let lock = lockfile_with(
            "linux-64-cuda",
            Platform::Linux64,
            vec!["__cuda=12.0".to_string()],
        );

        let aligned = align_platform_names(lock, &manifest);

        assert!(
            aligned.platform("gpu-linux").is_some(),
            "renamed entry should be queryable under the workspace name",
        );
        assert!(
            aligned.platform("linux-64-cuda").is_none(),
            "the old name must be gone after the rename",
        );
    }

    /// Lockfile entries whose identity doesn't match anything in the
    /// manifest pass through unchanged -- the satisfiability layer is the
    /// right place to flag the mismatch.
    #[test]
    fn rename_leaves_unmatched_entries_alone() {
        let manifest = manifest(
            r#"
            [workspace]
            name = "demo"
            channels = []
            platforms = ["linux-64"]
            "#,
        );
        let lock = lockfile_with(
            "leftover",
            Platform::Linux64,
            vec!["__cuda=12.0".to_string()],
        );

        let aligned = align_platform_names(lock, &manifest);

        // The manifest has no `__cuda` entry, so no rename can apply.
        assert!(aligned.platform("leftover").is_some());
    }

    /// If a manifest rename would clobber a name already present in the
    /// lockfile, the rename must be skipped so we don't lose a row.
    #[test]
    fn rename_skips_when_target_name_is_already_taken() {
        // Workspace has two entries that both target linux-64 -- one with
        // cuda, one bare -- and a lockfile that already has an entry
        // named "gpu-linux". Renaming "linux-64-cuda" to "gpu-linux"
        // would collide with the existing row.
        let manifest = manifest(
            r#"
            [workspace]
            name = "demo"
            channels = []
            platforms = [
              "linux-64",
              { name = "gpu-linux", platform = "linux-64", cuda = "12.0" },
            ]
            "#,
        );
        let mut builder = LockFile::builder()
            .with_platforms(vec![
                PlatformData {
                    name: PlatformName::try_from("gpu-linux").unwrap(),
                    subdir: Platform::Linux64,
                    virtual_packages: vec!["__cuda=11.0".to_string()],
                },
                PlatformData {
                    name: PlatformName::try_from("linux-64-cuda").unwrap(),
                    subdir: Platform::Linux64,
                    virtual_packages: vec!["__cuda=12.0".to_string()],
                },
            ])
            .unwrap();
        builder.set_channels("default", Vec::<rattler_lock::Channel>::new());
        builder.set_options("default", rattler_lock::SolveOptions::default());
        let lock = builder.finish();

        let aligned = align_platform_names(lock, &manifest);

        // Both rows survive under their original names: the colliding
        // rename was skipped rather than overwriting.
        assert!(aligned.platform("gpu-linux").is_some());
        assert!(aligned.platform("linux-64-cuda").is_some());
    }

    /// Lockfile entries that store extra subdir-default virtual packages
    /// (or none at all) still match a workspace entry whose customised
    /// identity is the same -- the defaults are filtered on both sides
    /// before comparing.
    #[test]
    fn rename_ignores_subdir_defaults_when_matching() {
        let manifest = manifest(
            r#"
            [workspace]
            name = "demo"
            channels = []
            platforms = [{ name = "gpu-linux", platform = "linux-64", cuda = "12.0" }]
            "#,
        );
        // Older lockfile that materialised the linux-64 defaults alongside
        // the user's __cuda; the rename pass should treat the entry as
        // identity-equal to the manifest's `gpu-linux`.
        let lock = lockfile_with(
            "linux-64-cuda",
            Platform::Linux64,
            vec![
                "__cuda=12.0".to_string(),
                "__linux=4.18".to_string(),
                "__glibc=2.28".to_string(),
                "__archspec=0=x86_64".to_string(),
                "__unix=0=0".to_string(),
            ],
        );

        let aligned = align_platform_names(lock, &manifest);

        assert!(aligned.platform("gpu-linux").is_some());
        assert!(aligned.platform("linux-64-cuda").is_none());
    }
}
