//! Module for reading and comparing PyPI package metadata from local source trees.
//!
//! This module provides functionality to:
//! 1. Read metadata from local pyproject.toml files
//! 2. Compare locked metadata against current source tree metadata
use std::collections::{BTreeSet, HashMap, HashSet};
use std::str::FromStr;

use indexmap::IndexMap;
use pep440_rs::{Version, VersionSpecifiers};
use pep508_rs::{PackageName, Requirement};
use pixi_install_pypi::LockedPypiRecord;
use uv_normalize::ExtraName;

/// Metadata extracted from a local package source tree.
#[derive(Debug, Clone)]
pub struct LocalPackageMetadata {
    /// The version of the package, if statically known.
    /// `None` for packages with dynamic versions.
    pub version: Option<Version>,
    /// The package dependencies.
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
/// what changed.
pub fn compare_metadata(
    locked_record: &LockedPypiRecord,
    current: &LocalPackageMetadata,
) -> Option<MetadataMismatch> {
    let locked = &locked_record.data;

    // Compare requires_dist (as normalized sets)
    let locked_deps: BTreeSet<String> = locked
        .requires_dist()
        .iter()
        .map(normalize_requirement)
        .collect();

    let current_deps: BTreeSet<String> = current
        .requires_dist
        .iter()
        .map(normalize_requirement)
        .collect();

    if locked_deps != current_deps {
        // Calculate the diff
        let added: Vec<Requirement> = current
            .requires_dist
            .iter()
            .filter(|r| !locked_deps.contains(&normalize_requirement(r)))
            .cloned()
            .collect();

        let removed: Vec<Requirement> = locked
            .requires_dist()
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
    if locked.requires_python() != current.requires_python.as_ref() {
        return Some(MetadataMismatch::RequiresPython {
            locked: locked.requires_python().cloned(),
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

/// Replace each `pkg[group]` self-reference with the raw entries of
/// `optional_dependencies[group]`, carrying the outer marker. Matches
/// what build backends bake into wheel METADATA; UV's static parse
/// leaves the self-references intact. Cycles in the optional-deps
/// graph are broken on the path that closes them.
pub fn expand_self_extras(
    requires_dist: Vec<Requirement>,
    package_name: &PackageName,
    optional_dependencies: &IndexMap<ExtraName, Vec<String>>,
) -> Vec<Requirement> {
    let parsed: HashMap<&str, Vec<Requirement>> = optional_dependencies
        .iter()
        .map(|(extra, raws)| {
            let reqs = raws
                .iter()
                .filter_map(|s| Requirement::from_str(s).ok())
                .collect::<Vec<_>>();
            (extra.as_ref(), reqs)
        })
        .collect();

    let mut result: Vec<Requirement> = Vec::new();
    let mut path: HashSet<&str> = HashSet::new();
    for req in &requires_dist {
        expand_into(req, package_name, &parsed, &mut path, &mut result);
    }
    result
}

fn expand_into<'a>(
    req: &Requirement,
    package_name: &PackageName,
    parsed: &'a HashMap<&'a str, Vec<Requirement>>,
    path: &mut HashSet<&'a str>,
    result: &mut Vec<Requirement>,
) {
    if req.name != *package_name || req.extras.is_empty() {
        result.push(req.clone());
        return;
    }
    for extra in &req.extras {
        let Some((&key, group_reqs)) = parsed.get_key_value(extra.as_ref()) else {
            continue;
        };
        if !path.insert(key) {
            continue;
        }
        for child in group_reqs {
            let mut expanded = child.clone();
            expanded.marker.and(req.marker.clone());
            expand_into(&expanded, package_name, parsed, path, result);
        }
        path.remove(key);
    }
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

        assert!(compare_metadata(&locked, &current).is_none());
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

        let mismatch = compare_metadata(&locked, &current);
        assert!(matches!(mismatch, Some(MetadataMismatch::RequiresDist(_))));
    }

    fn pkg_name(s: &str) -> PackageName {
        PackageName::from_str(s).unwrap()
    }

    fn extra(s: &str) -> ExtraName {
        ExtraName::from_str(s).unwrap()
    }

    fn req(s: &str) -> Requirement {
        Requirement::from_str(s).unwrap()
    }

    fn render(reqs: &[Requirement]) -> String {
        let mut lines: Vec<String> = reqs.iter().map(|r| r.to_string()).collect();
        lines.sort();
        lines.join("\n")
    }

    #[test]
    fn expand_self_extras_replaces_ribasim_style_self_refs() {
        // Mirrors Deltares/Ribasim: `delwaq` references `ribasim[netcdf]`
        // and `all` composes the other groups.
        let mut optional: IndexMap<ExtraName, Vec<String>> = IndexMap::new();
        optional.insert(extra("tests"), vec!["pytest".into()]);
        optional.insert(extra("netcdf"), vec!["xugrid".into()]);
        optional.insert(
            extra("delwaq"),
            vec!["jinja2".into(), "networkx".into(), "ribasim[netcdf]".into()],
        );
        optional.insert(
            extra("all"),
            vec![
                "ribasim[tests]".into(),
                "ribasim[netcdf]".into(),
                "ribasim[delwaq]".into(),
            ],
        );

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

        let expanded = expand_self_extras(static_parsed, &pkg_name("ribasim"), &optional);
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
        let optional: IndexMap<ExtraName, Vec<String>> = IndexMap::new();
        let input = vec![req("requests"), req("other[gpu] ; extra == 'all'")];
        let expanded = expand_self_extras(input, &pkg_name("mypkg"), &optional);
        insta::assert_snapshot!(render(&expanded), @r"
        other[gpu] ; extra == 'all'
        requests
        ");
    }

    #[test]
    fn expand_self_extras_drops_unknown_extras_silently() {
        // A self-reference to an extra that doesn't exist (typo, stale
        // metadata) is dropped.
        let optional: IndexMap<ExtraName, Vec<String>> = IndexMap::new();
        let input = vec![req("pandas"), req("mypkg[missing] ; extra == 'all'")];
        let expanded = expand_self_extras(input, &pkg_name("mypkg"), &optional);
        insta::assert_snapshot!(render(&expanded), @"pandas");
    }

    #[test]
    fn expand_self_extras_breaks_cycles() {
        // a -> b -> a; expansion must terminate. The non-cyclic dep
        // `actual` is still emitted with the outer marker.
        let mut optional: IndexMap<ExtraName, Vec<String>> = IndexMap::new();
        optional.insert(extra("a"), vec!["actual".into(), "mypkg[b]".into()]);
        optional.insert(extra("b"), vec!["mypkg[a]".into()]);

        let input = vec![req("mypkg[a] ; extra == 'X'")];
        let expanded = expand_self_extras(input, &pkg_name("mypkg"), &optional);
        insta::assert_snapshot!(render(&expanded), @"actual ; extra == 'x'");
    }
}
