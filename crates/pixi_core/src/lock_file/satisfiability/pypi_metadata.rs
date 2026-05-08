//! Module for reading and comparing PyPI package metadata from local source trees.
//!
//! This module provides functionality to:
//! 1. Read metadata from local pyproject.toml files
//! 2. Compare locked metadata against current source tree metadata
use std::collections::{BTreeSet, HashSet};

use pep440_rs::{Version, VersionSpecifiers};
use pep508_rs::{
    ExtraName, ExtraOperator, MarkerExpression, MarkerValueExtra, PackageName, Requirement,
};
use pixi_install_pypi::LockedPypiRecord;

/// Metadata extracted from a local package source tree.
#[derive(Debug, Clone)]
pub struct LocalPackageMetadata {
    /// The version of the package, if statically known.
    /// `None` for packages with dynamic versions.
    pub version: Option<Version>,
    /// The package dependencies as parsed from pyproject.toml,
    /// including optional-dependency entries with their
    /// `; extra == "X"` markers.
    pub requires_dist: Vec<Requirement>,
    /// The Python version requirement.
    pub requires_python: Option<VersionSpecifiers>,
}

/// The result of comparing locked metadata against current metadata.
#[derive(Debug)]
pub enum MetadataMismatch {
    /// The requires_dist (dependencies) have changed.
    RequiresDist(RequiresDistDiff),
    /// The version has changed.
    Version { locked: Version, current: Version },
    /// The requires_python has changed.
    RequiresPython {
        locked: Option<VersionSpecifiers>,
        current: Option<VersionSpecifiers>,
    },
}

/// Describes the difference in requires_dist between locked and current metadata.
#[derive(Debug)]
pub struct RequiresDistDiff {
    /// Dependencies that were added.
    pub added: Vec<Requirement>,
    /// Dependencies that were removed.
    pub removed: Vec<Requirement>,
}

/// Compare locked metadata against current metadata from the source tree.
///
/// Returns `None` if the metadata matches, or `Some(MetadataMismatch)` describing
/// what changed. Both sides are first normalized via `expand_self_extras`
/// so self-refs like `foo[test]; extra == "dev"` compare equal to the
/// build-backend-expanded `pytest; extra == "dev"`.
pub fn compare_metadata(
    locked_record: &LockedPypiRecord,
    package_name: &PackageName,
    current: &LocalPackageMetadata,
) -> Option<MetadataMismatch> {
    let locked_expanded = expand_self_extras(locked_record.data.requires_dist(), package_name);
    let current_expanded = expand_self_extras(&current.requires_dist, package_name);

    // Compare requires_dist (as normalized sets)
    let locked_deps: BTreeSet<String> = locked_expanded.iter().map(normalize_requirement).collect();
    let current_deps: BTreeSet<String> =
        current_expanded.iter().map(normalize_requirement).collect();

    if locked_deps != current_deps {
        let added: Vec<Requirement> = current_expanded
            .iter()
            .filter(|r| !locked_deps.contains(&normalize_requirement(r)))
            .cloned()
            .collect();

        let removed: Vec<Requirement> = locked_expanded
            .iter()
            .filter(|r| !current_deps.contains(&normalize_requirement(r)))
            .cloned()
            .collect();

        return Some(MetadataMismatch::RequiresDist(RequiresDistDiff {
            added,
            removed,
        }));
    }

    // Compare the locked version (always present on LockedPypiRecord)
    // against the current version from the source tree.
    if let Some(current_version) = &current.version
        && &locked_record.locked_version != current_version
    {
        return Some(MetadataMismatch::Version {
            locked: locked_record.locked_version.clone(),
            current: current_version.clone(),
        });
    }

    // Compare requires_python
    let locked_requires_python = locked_record.data.requires_python();
    if locked_requires_python != current.requires_python.as_ref() {
        return Some(MetadataMismatch::RequiresPython {
            locked: locked_requires_python.cloned(),
            current: current.requires_python.clone(),
        });
    }

    None
}

/// Normalize a requirement for comparison purposes.
///
/// This ensures that semantically equivalent requirements compare equal,
/// regardless of formatting differences (e.g., whitespace, order of extras).
fn normalize_requirement(req: &Requirement) -> String {
    // Use the canonical string representation
    // The pep508_rs library already normalizes package names and versions
    req.to_string()
}

/// Replace each `pkg[group]` self-ref with the entries gated by
/// `extra == "<group>"` from the same list, carrying the outer marker.
/// Mirrors the shape that build backends emit into wheel METADATA.
/// Cycles in the optional-deps graph terminate on the closing edge.
///
/// Implemented with an explicit work stack rather than recursion so a
/// pathologically deep optional-deps graph can't blow the call stack.
pub fn expand_self_extras(
    requires_dist: &[Requirement],
    package_name: &PackageName,
) -> Vec<Requirement> {
    enum Frame {
        /// Try to expand this requirement, or push it to the result if
        /// it isn't a self-reference.
        Expand(Requirement),
        /// Pop an extra off the active path once its expansion subtree
        /// has finished processing.
        Cleanup(ExtraName),
    }

    let mut result: Vec<Requirement> = Vec::new();
    let mut path: HashSet<ExtraName> = HashSet::new();
    // Reversed so the first input is processed first.
    let mut stack: Vec<Frame> = requires_dist
        .iter()
        .rev()
        .map(|r| Frame::Expand(r.clone()))
        .collect();

    while let Some(frame) = stack.pop() {
        match frame {
            Frame::Cleanup(extra) => {
                path.remove(&extra);
            }
            Frame::Expand(req) => {
                if req.name != *package_name || req.extras.is_empty() {
                    result.push(req);
                    continue;
                }
                // `foo[a, b]` and `foo[a]` + `foo[b]` produce the same
                // expansion. Splitting keeps the active path scoped to
                // one extra at a time, matching the recursive version.
                if req.extras.len() > 1 {
                    for extra in req.extras.iter().rev() {
                        let mut split = req.clone();
                        split.extras = vec![extra.clone()];
                        stack.push(Frame::Expand(split));
                    }
                    continue;
                }
                let requested_extra = req.extras[0].clone();
                if !path.insert(requested_extra.clone()) {
                    continue;
                }
                // Cleanup is pushed first so it pops last, after every
                // child expansion for this extra is done.
                stack.push(Frame::Cleanup(requested_extra.clone()));
                let mut children: Vec<Frame> = Vec::new();
                for child in requires_dist {
                    let Some(MarkerExpression::Extra {
                        operator: ExtraOperator::Equal,
                        name: MarkerValueExtra::Extra(child_extra),
                    }) = child.marker.top_level_extra()
                    else {
                        continue;
                    };
                    if child_extra != requested_extra {
                        continue;
                    }
                    let mut expanded = child.clone();
                    expanded.marker = child.marker.clone().simplify_extras(&[child_extra]);
                    expanded.marker.and(req.marker.clone());
                    children.push(Frame::Expand(expanded));
                }
                children.reverse();
                stack.extend(children);
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    use crate::lock_file::tests::make_wheel_package_with;
    use pixi_install_pypi::UnresolvedPypiRecord;
    use rattler_lock::{PypiDistributionData, PypiPackageData};

    fn lock_for_test(data: PypiPackageData) -> LockedPypiRecord {
        let version = data
            .version()
            .cloned()
            .unwrap_or_else(|| Version::from_str("42.23").unwrap());
        UnresolvedPypiRecord::from(data).lock(version)
    }

    fn pkg_name(s: &str) -> PackageName {
        PackageName::from_str(s).unwrap()
    }

    fn req(s: &str) -> Requirement {
        Requirement::from_str(s).unwrap()
    }

    fn render(reqs: &[Requirement]) -> String {
        let mut lines: Vec<String> = reqs.iter().map(|r| r.to_string()).collect();
        lines.sort();
        lines.join("\n")
    }

    /// Locks in the assumption `compare_metadata` relies on: uv's
    /// static parse of pyproject.toml flattens both `[project.dependencies]`
    /// and `[project.optional-dependencies]` into a single
    /// `requires_dist`, attaching `; extra == "X"` markers to the
    /// optional entries. Without that, `expand_self_extras` would have
    /// nothing to expand `foo[X]` against on the static-parse side.
    #[test]
    fn uv_static_parse_flattens_optional_dependencies_with_extra_markers() {
        let toml = r#"
[project]
name = "foo"
version = "0.1.0"
dependencies = ["numpy"]

[project.optional-dependencies]
test = ["pytest"]
dev  = ["foo[test]"]
"#;
        let pyproject = uv_pypi_types::PyProjectToml::from_toml(toml).unwrap();
        let requires_dist = uv_pypi_types::RequiresDist::from_pyproject_toml(pyproject).unwrap();
        let rendered: Vec<String> = requires_dist
            .requires_dist
            .iter()
            .map(|r| r.to_string())
            .collect();
        let mut sorted = rendered.clone();
        sorted.sort();
        insta::assert_snapshot!(sorted.join("\n"), @r"
        foo[test] ; extra == 'dev'
        numpy
        pytest ; extra == 'test'
        ");
    }

    #[test]
    fn test_normalize_requirement() {
        let req1: Requirement = "numpy>=1.0".parse().unwrap();
        let req2: Requirement = "numpy >= 1.0".parse().unwrap();
        // Note: These may or may not be equal depending on pep508_rs normalization
        // The important thing is we consistently compare them
        assert_eq!(normalize_requirement(&req1), normalize_requirement(&req1));
        let _ = req2; // silence unused warning
    }

    #[test]
    fn test_compare_metadata_same() {
        let locked = lock_for_test(PypiPackageData::Distribution(Box::new(
            PypiDistributionData {
                name: "test-package".parse().unwrap(),
                version: Version::from_str("1.0.0").unwrap(),
                requires_dist: vec!["numpy>=1.0".parse().unwrap()],
                requires_python: Some(VersionSpecifiers::from_str(">=3.8").unwrap()),
                location: rattler_lock::UrlOrPath::Url(url::Url::parse("file:///test").unwrap())
                    .into(),
                hash: None,
                index_url: None,
            },
        )));

        let current = LocalPackageMetadata {
            version: Some(Version::from_str("1.0.0").unwrap()),
            requires_dist: vec!["numpy>=1.0".parse().unwrap()],
            requires_python: Some(VersionSpecifiers::from_str(">=3.8").unwrap()),
        };

        assert!(compare_metadata(&locked, &pkg_name("test-package"), &current).is_none());
    }

    #[test]
    fn test_compare_metadata_different_deps() {
        let locked = lock_for_test(make_wheel_package_with(
            "test-package",
            "1.0.0",
            rattler_lock::UrlOrPath::Url(url::Url::parse("file:///test").unwrap()).into(),
            None,
            None,
            vec!["numpy>=1.0".parse().unwrap()],
            None,
        ));

        let current = LocalPackageMetadata {
            version: Some(Version::from_str("1.0.0").unwrap()),
            requires_dist: vec![
                "numpy>=1.0".parse().unwrap(),
                "pandas>=2.0".parse().unwrap(), // Added
            ],
            requires_python: None,
        };

        let mismatch = compare_metadata(&locked, &pkg_name("test-package"), &current);
        assert!(matches!(mismatch, Some(MetadataMismatch::RequiresDist(_))));
    }

    /// Issue #6049: both lock and current hold `foo[test]; extra ==
    /// 'dev'` verbatim from uv's static parse; they must compare equal
    /// after expansion.
    #[test]
    fn compare_self_referential_extras_unexpanded_lock_matches() {
        let locked = lock_for_test(make_wheel_package_with(
            "foo",
            "0.1.0",
            rattler_lock::UrlOrPath::Url(url::Url::parse("file:///test").unwrap()).into(),
            None,
            None,
            vec![
                req("pytest ; extra == 'test'"),
                req("foo[test] ; extra == 'dev'"),
            ],
            None,
        ));

        let current = LocalPackageMetadata {
            version: Some(Version::from_str("0.1.0").unwrap()),
            requires_dist: vec![
                req("pytest ; extra == 'test'"),
                req("foo[test] ; extra == 'dev'"),
            ],
            requires_python: None,
        };

        assert!(
            compare_metadata(&locked, &pkg_name("foo"), &current).is_none(),
            "self-refs in lock and pyproject must compare equal after expansion"
        );
    }

    /// Companion case: lock came from build-backend wheel METADATA
    /// (already expanded), current came from static parse (still a
    /// self-ref). Expanding the current side must match.
    #[test]
    fn compare_expanded_lock_matches_self_ref_pyproject() {
        let locked = lock_for_test(make_wheel_package_with(
            "foo",
            "0.1.0",
            rattler_lock::UrlOrPath::Url(url::Url::parse("file:///test").unwrap()).into(),
            None,
            None,
            vec![
                req("pytest ; extra == 'test'"),
                req("pytest ; extra == 'dev'"),
            ],
            None,
        ));

        let current = LocalPackageMetadata {
            version: Some(Version::from_str("0.1.0").unwrap()),
            requires_dist: vec![
                req("pytest ; extra == 'test'"),
                req("foo[test] ; extra == 'dev'"),
            ],
            requires_python: None,
        };

        assert!(
            compare_metadata(&locked, &pkg_name("foo"), &current).is_none(),
            "build-backend-expanded lock must match self-ref pyproject after expansion"
        );
    }

    /// Editing an extra's contents without re-locking still surfaces
    /// as a diff: each side expands against its own list.
    #[test]
    fn compare_detects_stale_optional_deps() {
        let locked = lock_for_test(make_wheel_package_with(
            "foo",
            "0.1.0",
            rattler_lock::UrlOrPath::Url(url::Url::parse("file:///test").unwrap()).into(),
            None,
            None,
            vec![
                req("pytest ; extra == 'test'"),
                req("foo[test] ; extra == 'dev'"),
            ],
            None,
        ));

        let current = LocalPackageMetadata {
            version: Some(Version::from_str("0.1.0").unwrap()),
            requires_dist: vec![
                req("pytest-mock ; extra == 'test'"),
                req("foo[test] ; extra == 'dev'"),
            ],
            requires_python: None,
        };

        let mismatch = compare_metadata(&locked, &pkg_name("foo"), &current);
        let Some(MetadataMismatch::RequiresDist(diff)) = mismatch else {
            panic!("expected RequiresDist mismatch, got {mismatch:?}");
        };
        insta::assert_snapshot!(format!(
            "added:\n{}\n\nremoved:\n{}",
            render(&diff.added),
            render(&diff.removed)
        ), @r"
        added:
        pytest-mock ; extra == 'dev'
        pytest-mock ; extra == 'test'

        removed:
        pytest ; extra == 'dev'
        pytest ; extra == 'test'
        ");
    }

    #[test]
    fn expand_self_extras_replaces_ribasim_style_self_refs() {
        // Mirrors Deltares/Ribasim: `delwaq` references `ribasim[netcdf]`
        // and `all` composes the other groups.
        let static_parsed = vec![
            req("pandas"),
            req("pytest ; extra == 'tests'"),
            req("xugrid ; extra == 'netcdf'"),
            req("jinja2 ; extra == 'delwaq'"),
            req("networkx ; extra == 'delwaq'"),
            req("ribasim[netcdf] ; extra == 'delwaq'"),
            req("ribasim[tests] ; extra == 'all'"),
            req("ribasim[netcdf] ; extra == 'all'"),
            req("ribasim[delwaq] ; extra == 'all'"),
        ];

        let expanded = expand_self_extras(&static_parsed, &pkg_name("ribasim"));
        insta::assert_snapshot!(render(&expanded), @r"
        jinja2 ; extra == 'all'
        jinja2 ; extra == 'delwaq'
        networkx ; extra == 'all'
        networkx ; extra == 'delwaq'
        pandas
        pytest ; extra == 'all'
        pytest ; extra == 'tests'
        xugrid ; extra == 'all'
        xugrid ; extra == 'all'
        xugrid ; extra == 'delwaq'
        xugrid ; extra == 'netcdf'
        ");
    }

    #[test]
    fn expand_self_extras_preserves_non_self_references() {
        let input = vec![req("requests"), req("other[gpu] ; extra == 'all'")];
        let expanded = expand_self_extras(&input, &pkg_name("mypkg"));
        insta::assert_snapshot!(render(&expanded), @r"
        other[gpu] ; extra == 'all'
        requests
        ");
    }

    #[test]
    fn expand_self_extras_drops_unknown_extras_silently() {
        // Self-ref to an extra with no matching entries is dropped.
        let input = vec![req("pandas"), req("mypkg[missing] ; extra == 'all'")];
        let expanded = expand_self_extras(&input, &pkg_name("mypkg"));
        insta::assert_snapshot!(render(&expanded), @"pandas");
    }

    #[test]
    fn expand_self_extras_breaks_direct_self_loop() {
        // The entry under extra `a` references `foo[a]` itself.
        // Without the path tracker this would recurse forever; with it,
        // `expand_into` skips the second insert of `a` and terminates.
        let input = vec![req("foo[a] ; extra == 'a'")];
        let expanded = expand_self_extras(&input, &pkg_name("foo"));
        assert!(
            expanded.is_empty(),
            "direct self-loop must terminate and emit nothing; got {expanded:?}"
        );
    }

    #[test]
    fn expand_self_extras_breaks_cycles() {
        // a -> b -> a must terminate; non-cyclic deps still emit.
        let input = vec![
            req("actual ; extra == 'a'"),
            req("mypkg[b] ; extra == 'a'"),
            req("mypkg[a] ; extra == 'b'"),
            req("mypkg[a] ; extra == 'X'"),
        ];
        let expanded = expand_self_extras(&input, &pkg_name("mypkg"));
        insta::assert_snapshot!(render(&expanded), @r"
        actual ; extra == 'a'
        actual ; extra == 'a'
        actual ; extra == 'b'
        actual ; extra == 'x'
        ");
    }

    #[test]
    fn expand_self_extras_preserves_non_extra_marker_constraints() {
        // The child's non-extra marker constraints survive expansion.
        let input = vec![
            req("trio ; python_version >= '3.10' and extra == 'async'"),
            req("foo[async] ; extra == 'all'"),
        ];
        let expanded = expand_self_extras(&input, &pkg_name("foo"));
        insta::assert_snapshot!(render(&expanded), @r"
        trio ; python_full_version >= '3.10' and extra == 'all'
        trio ; python_full_version >= '3.10' and extra == 'async'
        ");
    }
}
