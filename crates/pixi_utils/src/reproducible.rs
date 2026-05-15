//! Stamp the pixi-owned subset of a workspace's `.pixi/` tree with a
//! deterministic modification time so repeated installs of the same
//! workspace produce a bit-identical layout.
//!
//! Reproducibility is opt-in: the entire pass is a no-op unless
//! `SOURCE_DATE_EPOCH` is set to a parseable Unix-seconds integer (the
//! reproducible-builds convention). Without it we fall back to whatever
//! mtimes the OS hands out, matching the historical behavior.
//!
//! ## Ownership rule
//!
//! An entry under `<workspace>/.pixi/` is stamped iff one of:
//!
//! 1. it is `.pixi/` itself, or any path that is **not** below
//!    `.pixi/envs/`,
//! 2. it is `.pixi/envs/` itself,
//! 3. it is `.pixi/envs/<env>/` itself,
//! 4. it is `.pixi/envs/<env>/CACHEDIR.TAG`,
//! 5. it is `.pixi/envs/<env>/conda-meta/` or anything (recursively)
//!    inside it,
//! 6. it is **any directory** elsewhere under `.pixi/envs/<env>/`
//!    (e.g., `bin/`, `lib/`, `share/`, `share/info/`, …).
//!
//! Files and symlinks under `.pixi/envs/<env>/` outside `conda-meta/`
//! and `CACHEDIR.TAG` are left untouched -- those are extracted package
//! contents, already stamped by rattler with a stable per-package mtime
//! derived from each package's `info/about.json`. The directories that
//! contain them, however, are scaffolding rattler creates with a
//! wall-clock mtime; pixi clamps those to `SOURCE_DATE_EPOCH` because
//! rattler doesn't currently stamp the directories themselves.
//!
//! ## Order
//!
//! The walk visits children before their parent so a child stamp can't
//! re-bump an already-stamped parent's mtime via the kernel's "child
//! created/modified ⇒ parent mtime updated" semantics.

use std::path::Path;

use filetime::FileTime;
use fs_err as fs;

/// Standard reproducible-builds env var. See
/// <https://reproducible-builds.org/docs/source-date-epoch/>.
const SOURCE_DATE_EPOCH: &str = "SOURCE_DATE_EPOCH";

/// 1980-01-01 00:00:00 UTC. The earliest timestamp representable on FAT
/// and exFAT, so we floor `SOURCE_DATE_EPOCH` here to keep the stamp pass
/// portable. The exact timestamp doesn't matter -- only stability does.
const MIN_SAFE_EPOCH: i64 = 315_532_800;

/// The deterministic modification time to use, derived from
/// `SOURCE_DATE_EPOCH`. Returns `None` when the variable is unset, empty,
/// or unparsable. Otherwise the parsed value is clamped to 1980-01-01
/// (the FAT/exFAT epoch floor) before being returned.
pub fn reproducible_mtime() -> Option<FileTime> {
    let raw = std::env::var(SOURCE_DATE_EPOCH).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let secs = match trimmed.parse::<i64>() {
        Ok(secs) => secs,
        Err(err) => {
            tracing::warn!(
                "Ignoring malformed {SOURCE_DATE_EPOCH}={raw:?}: {err}; \
                 leaving mtimes unset for this install."
            );
            return None;
        }
    };
    Some(FileTime::from_unix_time(secs.max(MIN_SAFE_EPOCH), 0))
}

/// Stamp the pixi-owned entries under `pixi_dir` with `mtime` (see the
/// ownership rule documented at the module level).
///
/// `pixi_dir` should be the workspace's top-level `.pixi/` directory. If
/// it doesn't exist, this returns `Ok(())` -- pixi may invoke us before a
/// fresh workspace has anything to stamp. Any entry the walker encounters
/// that has already been removed is silently skipped so transient mid-
/// install file shuffling (atomic renames, etc.) doesn't surface as an
/// error.
pub fn stamp_pixi_tree(pixi_dir: &Path, mtime: FileTime) -> std::io::Result<()> {
    match fs::symlink_metadata(pixi_dir) {
        Ok(meta) if meta.file_type().is_dir() => {}
        Ok(_) => return Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    }
    walk_and_stamp(pixi_dir, pixi_dir, mtime)
}

/// Recursive worker. `root` is the workspace `.pixi/` directory we
/// classify file paths against; `path` is the entry currently being
/// visited.
///
/// Directories under `.pixi/` are always stamped (after recursing into
/// their children, so a child stamp doesn't bump the parent's mtime
/// after we set it). Files and symlinks need the classification check
/// because rattler's per-package mtimes on extracted package contents
/// must not be disturbed.
fn walk_and_stamp(root: &Path, path: &Path, mtime: FileTime) -> std::io::Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        // Vanished between read_dir and us -- accept it as a no-op.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };

    if meta.file_type().is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            walk_and_stamp(root, &entry.path(), mtime)?;
        }
        stamp(path, &meta, mtime)?;
    } else if should_stamp_file(root, path) {
        stamp(path, &meta, mtime)?;
    }

    Ok(())
}

/// Apply `mtime` to a single entry, treating a vanished path as success
/// (mid-install file shuffling can briefly hide entries from us).
fn stamp(path: &Path, meta: &std::fs::Metadata, mtime: FileTime) -> std::io::Result<()> {
    // `filetime` has no `set_symlink_file_mtime`, so for symlinks we
    // pass the same value for atime -- keeps both fields stable. For
    // regular files `set_file_mtime` leaves atime alone via UTIME_OMIT.
    let result = if meta.file_type().is_symlink() {
        filetime::set_symlink_file_times(path, mtime, mtime)
    } else {
        filetime::set_file_mtime(path, mtime)
    };
    match result {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// Decide whether a non-directory entry should have its mtime stamped.
/// Pure path classification -- never touches the disk. Directories don't
/// reach here; the walker stamps them unconditionally.
fn should_stamp_file(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        // Outside the pixi root -- must be a walker bug; refuse to touch.
        return false;
    };

    let mut comps = rel.components();
    // A non-directory at the `.pixi/` root would be impossible (it's a
    // dir by definition), but treat the empty case conservatively.
    let Some(first) = comps.next() else {
        return false;
    };
    // Top-level files outside `envs/` are always pixi-owned
    // (`.gitignore`, `.condapackageignore`, future workspace metadata).
    if first.as_os_str() != "envs" {
        return true;
    }

    // Below `.pixi/envs/`. Files only land at depth >= 3 in practice
    // (`envs/<env>/<entry>`); any shallower path here is some odd file
    // that mirrors a directory name (`envs` itself, etc.) -- leave alone.
    let (Some(_env), Some(third)) = (comps.next(), comps.next()) else {
        return false;
    };

    let third_name = third.as_os_str();
    let has_more = comps.next().is_some();

    if !has_more {
        // `.pixi/envs/<env>/<third>` direct child file. The only file
        // pixi owns here is `CACHEDIR.TAG`.
        third_name == "CACHEDIR.TAG"
    } else {
        // `.pixi/envs/<env>/<third>/...` deeper file. Pixi owns the
        // entire conda-meta subtree; everything else is package content.
        third_name == "conda-meta"
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use filetime::FileTime;
    use fs_err as fs;
    use tempfile::TempDir;

    use super::*;

    /// One row of the fixture: relative path under `.pixi/`, whether
    /// it's a directory, and whether the stamp pass should clamp its
    /// mtime. `""` denotes `.pixi/` itself.
    type FixtureEntry = (&'static str, bool, bool);

    /// Build a tree with the same shape the pixi installer produces so
    /// the stamp pass sees realistic paths and types. Returns the
    /// tempdir and a vector of `(absolute_path, is_dir, expected_stamp)`.
    fn make_fixture() -> (TempDir, Vec<(PathBuf, bool, bool)>) {
        let tmp = TempDir::new().unwrap();
        let pixi_dir = tmp.path().join(".pixi");

        // is_dir is explicit so we never accidentally materialize `bat` (no
        // extension) as a directory.
        let entries: &[FixtureEntry] = &[
            // (rel-under-.pixi, is_dir, expected_stamp)
            // Directories: every one under `.pixi/` is stamped.
            ("", true, true),
            ("envs", true, true),
            ("envs/default", true, true),
            ("envs/default/conda-meta", true, true),
            ("envs/default/bin", true, true),
            ("envs/default/lib", true, true),
            ("envs/default/share", true, true),
            ("envs/default/share/info", true, true),
            // Top-level pixi files: stamped.
            (".gitignore", false, true),
            (".condapackageignore", false, true),
            // Prefix-root files: only `CACHEDIR.TAG` is pixi-owned.
            ("envs/default/CACHEDIR.TAG", false, true),
            // conda-meta contents: all stamped (rattler-written records
            // and pixi-written bookkeeping alike).
            ("envs/default/conda-meta/history", false, true),
            ("envs/default/conda-meta/bat-0.26.1.json", false, true),
            ("envs/default/conda-meta/pixi", false, true),
            ("envs/default/conda-meta/pixi_env_prefix", false, true),
            (
                "envs/default/conda-meta/.pixi-environment-fingerprint",
                false,
                true,
            ),
            // Extracted package files: rattler stamped them per-package,
            // pixi must not touch them.
            ("envs/default/bin/bat", false, false),
            ("envs/default/lib/libgomp.so.1.0.0", false, false),
            ("envs/default/share/info/libgomp.info", false, false),
        ];

        for (rel, is_dir, _) in entries {
            let path = if rel.is_empty() {
                pixi_dir.clone()
            } else {
                pixi_dir.join(rel)
            };
            if *is_dir {
                fs::create_dir_all(&path).unwrap();
            } else {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::write(&path, b"").unwrap();
            }
        }

        let absolute: Vec<_> = entries
            .iter()
            .map(|(rel, is_dir, stamped)| {
                let path = if rel.is_empty() {
                    pixi_dir.clone()
                } else {
                    pixi_dir.join(rel)
                };
                (path, *is_dir, *stamped)
            })
            .collect();
        (tmp, absolute)
    }

    /// Snapshot every entry's mtime so we can detect which ones changed.
    fn collect_mtimes(paths: &[(PathBuf, bool, bool)]) -> Vec<FileTime> {
        paths
            .iter()
            .map(|(p, _, _)| {
                let m = fs::symlink_metadata(p).unwrap();
                FileTime::from_last_modification_time(&m)
            })
            .collect()
    }

    #[test]
    fn stamp_touches_owned_entries_and_leaves_package_files_alone() {
        let (tmp, entries) = make_fixture();
        let pixi_dir = tmp.path().join(".pixi");

        // Pin every entry to a known "before" mtime so we can tell post-hoc
        // which ones the stamp pass touched.
        let untouched = FileTime::from_unix_time(1_700_000_000, 0);
        for (path, _, _) in &entries {
            filetime::set_file_mtime(path, untouched).unwrap();
        }

        let target = FileTime::from_unix_time(1_800_000_000, 0);
        stamp_pixi_tree(&pixi_dir, target).unwrap();

        for (path, _is_dir, expected_stamped) in &entries {
            let m = fs::symlink_metadata(path).unwrap();
            let actual = FileTime::from_last_modification_time(&m);
            let want = if *expected_stamped { target } else { untouched };
            assert_eq!(
                actual,
                want,
                "{} mtime should be {:?} (stamped={}), got {:?}",
                path.display(),
                want,
                expected_stamped,
                actual
            );
        }
    }

    #[test]
    fn idempotent_under_repeated_invocation() {
        let (tmp, entries) = make_fixture();
        let pixi_dir = tmp.path().join(".pixi");
        let target = FileTime::from_unix_time(1_800_000_000, 0);
        stamp_pixi_tree(&pixi_dir, target).unwrap();
        let after_first = collect_mtimes(&entries);
        stamp_pixi_tree(&pixi_dir, target).unwrap();
        let after_second = collect_mtimes(&entries);
        assert_eq!(after_first, after_second);
    }

    #[test]
    fn missing_pixi_dir_is_a_noop() {
        let tmp = TempDir::new().unwrap();
        let pixi_dir = tmp.path().join(".pixi");
        // Should not error, even though the dir doesn't exist.
        let target = FileTime::from_unix_time(1_800_000_000, 0);
        stamp_pixi_tree(&pixi_dir, target).unwrap();
    }

    #[test]
    fn reproducible_mtime_parses_source_date_epoch() {
        temp_env::with_var("SOURCE_DATE_EPOCH", Some("1700000000"), || {
            let mtime = reproducible_mtime().expect("should parse");
            assert_eq!(mtime, FileTime::from_unix_time(1_700_000_000, 0));
        });
    }

    #[test]
    fn reproducible_mtime_unset_returns_none() {
        temp_env::with_var("SOURCE_DATE_EPOCH", None::<&str>, || {
            assert!(reproducible_mtime().is_none());
        });
    }

    #[test]
    fn reproducible_mtime_malformed_returns_none() {
        temp_env::with_var("SOURCE_DATE_EPOCH", Some("not-a-number"), || {
            assert!(reproducible_mtime().is_none());
        });
    }

    #[test]
    fn reproducible_mtime_empty_returns_none() {
        temp_env::with_var("SOURCE_DATE_EPOCH", Some(""), || {
            assert!(reproducible_mtime().is_none());
        });
    }

    #[test]
    fn reproducible_mtime_at_floor_passes_through() {
        // Exactly 1980-01-01 -- the floor is inclusive.
        temp_env::with_var("SOURCE_DATE_EPOCH", Some("315532800"), || {
            assert_eq!(
                reproducible_mtime(),
                Some(FileTime::from_unix_time(MIN_SAFE_EPOCH, 0))
            );
        });
    }

    #[test]
    fn reproducible_mtime_pre_1980_clamps_up() {
        // Unix epoch (1970-01-01) is unrepresentable on FAT/exFAT;
        // clamp to the safe floor.
        temp_env::with_var("SOURCE_DATE_EPOCH", Some("0"), || {
            assert_eq!(
                reproducible_mtime(),
                Some(FileTime::from_unix_time(MIN_SAFE_EPOCH, 0))
            );
        });
    }

    #[test]
    fn reproducible_mtime_negative_clamps_up() {
        // A negative `SOURCE_DATE_EPOCH` (pre-1970) is even further out
        // of range; same clamp behavior.
        temp_env::with_var("SOURCE_DATE_EPOCH", Some("-1000"), || {
            assert_eq!(
                reproducible_mtime(),
                Some(FileTime::from_unix_time(MIN_SAFE_EPOCH, 0))
            );
        });
    }

    #[test]
    fn reproducible_mtime_above_floor_passes_through() {
        // A modern timestamp > floor stays as-is.
        temp_env::with_var("SOURCE_DATE_EPOCH", Some("1700000000"), || {
            assert_eq!(
                reproducible_mtime(),
                Some(FileTime::from_unix_time(1_700_000_000, 0))
            );
            // And it's strictly above the floor, so the clamp didn't
            // accidentally activate.
            assert!(reproducible_mtime().unwrap() > FileTime::from_unix_time(MIN_SAFE_EPOCH, 0));
        });
    }

    /// Pure-path coverage of `should_stamp_file`. Directories don't
    /// reach this function (the walker stamps them unconditionally), so
    /// only file paths appear here.
    #[test]
    fn should_stamp_file_pure_path_logic() {
        let root = Path::new("/ws/.pixi");
        let cases: &[(&str, bool)] = &[
            // Top-level files under `.pixi/`: stamped.
            ("/ws/.pixi/.gitignore", true),
            ("/ws/.pixi/.condapackageignore", true),
            // Prefix root: only `CACHEDIR.TAG` is owned.
            ("/ws/.pixi/envs/default/CACHEDIR.TAG", true),
            ("/ws/.pixi/envs/default/some_other_file.txt", false),
            // conda-meta subtree: all owned, at any depth.
            ("/ws/.pixi/envs/default/conda-meta/history", true),
            ("/ws/.pixi/envs/default/conda-meta/bat.json", true),
            ("/ws/.pixi/envs/default/conda-meta/sub/deeper", true),
            // Everything else under `<env>/` is package content.
            ("/ws/.pixi/envs/default/bin/bat", false),
            ("/ws/.pixi/envs/default/lib/libgomp.so", false),
            ("/ws/.pixi/envs/default/lib/libgomp.so.1.0.0", false),
            ("/ws/.pixi/envs/default/share/info/foo.info", false),
            ("/ws/.pixi/envs/default/random_other_dir/inside", false),
        ];
        for (s, want) in cases {
            let got = should_stamp_file(root, Path::new(s));
            assert_eq!(got, *want, "should_stamp_file({s}) = {got}, wanted {want}");
        }
    }
}
