# Multi-name `conda-pypi-map` Values Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mapping values in `conda-pypi-map` accept a single PyPI name, a list of PyPI names, or `null`/`false` — in JSON mapping files and inline TOML — so parselmouth's `files/v0/<channel>/compressed_mapping.json` format works directly.

**Architecture:** Introduce a `PypiNames(Vec<String>)` newtype with lenient serde (string → 1-element list, list → list, `null`/`[]` → empty list) and switch `CompressedMapping = HashMap<String, PypiNames>`. The three derivation outcomes map to: key absent → `NotApplicable`, empty list → `NoPurls`, non-empty → one purl per name. Spec: `docs/superpowers/specs/2026-06-12-conda-pypi-map-multi-name-design.md`.

**Tech Stack:** Rust (serde, toml_span, miette, insta), pydantic JSON schema (`schema/model.py`).

**Conventions:** Run all `cargo` commands from the repo root `/Users/graf/oss/pixi`. End every commit message with the line `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`. Snapshot acceptance: prefer `cargo insta test --accept -p <crate>`; if `cargo-insta` is not installed use `INSTA_UPDATE=always cargo test -p <crate>` then re-run normally.

---

### Task 1: `PypiNames` value type

A new module in `pypi_mapping` holding the value type with lenient serde. Purely additive — nothing else changes yet, the workspace stays green.

**Files:**
- Create: `crates/pypi_mapping/src/pypi_names.rs`
- Modify: `crates/pypi_mapping/src/lib.rs` (module declaration + re-export only)

- [ ] **Step 1: Write the type skeleton and the failing tests**

Create `crates/pypi_mapping/src/pypi_names.rs` with the type and tests, but **without** the serde impls yet:

```rust
//! [`PypiNames`] — the value of one conda-to-pypi mapping entry.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// The PyPI equivalents of one conda package.
///
/// Mapping documents spell this as a single name (`"numpy"`), a list of
/// names (`["airflow", "apache-airflow"]`), or `null` ("known not to be on
/// PyPI"). All forms normalize to a list; an empty list means the package
/// has no PyPI equivalent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PypiNames(pub Vec<String>);

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn parse(json: &str) -> HashMap<String, PypiNames> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn test_deserializes_single_name() {
        let mapping = parse(r#"{"numpy": "my-numpy"}"#);
        assert_eq!(mapping["numpy"], PypiNames(vec!["my-numpy".to_string()]));
    }

    #[test]
    fn test_deserializes_name_list() {
        let mapping = parse(r#"{"airflow": ["airflow", "apache-airflow"]}"#);
        assert_eq!(
            mapping["airflow"],
            PypiNames(vec!["airflow".to_string(), "apache-airflow".to_string()])
        );
    }

    #[test]
    fn test_deserializes_null_and_empty_list_as_not_on_pypi() {
        let mapping = parse(r#"{"a": null, "b": []}"#);
        assert_eq!(mapping["a"], PypiNames(Vec::new()));
        assert_eq!(mapping["b"], PypiNames(Vec::new()));
    }

    #[test]
    fn test_deserializes_mixed_document() {
        // Single-name, list and null entries may be mixed in one document.
        let mapping = parse(r#"{"a": "b", "c": ["d", "e"], "f": null}"#);
        assert_eq!(mapping["a"], PypiNames(vec!["b".to_string()]));
        assert_eq!(
            mapping["c"],
            PypiNames(vec!["d".to_string(), "e".to_string()])
        );
        assert_eq!(mapping["f"], PypiNames(Vec::new()));
    }

    #[test]
    fn test_rejects_non_string_values() {
        let err = serde_json::from_str::<HashMap<String, PypiNames>>(r#"{"a": 1}"#).unwrap_err();
        assert!(err.to_string().contains("a pypi name"), "{err}");
        assert!(serde_json::from_str::<HashMap<String, PypiNames>>(r#"{"a": ["b", 2]}"#).is_err());
    }

    #[test]
    fn test_serializes_as_list() {
        assert_eq!(
            serde_json::to_string(&PypiNames(vec!["b".to_string()])).unwrap(),
            r#"["b"]"#
        );
        assert_eq!(serde_json::to_string(&PypiNames(Vec::new())).unwrap(), "[]");
    }
}
```

Register the module in `crates/pypi_mapping/src/lib.rs`. After the existing line `mod purl;` (line 42) add:

```rust
mod pypi_names;
```

After the existing line `pub use purl::PurlDerivationSource;` (line 52) add:

```rust
pub use pypi_names::PypiNames;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p pypi_mapping pypi_names`
Expected: compile error — `PypiNames` does not implement `Deserialize`/`Serialize` (also unused-import warnings for the serde items).

- [ ] **Step 3: Implement the serde impls**

Add to `crates/pypi_mapping/src/pypi_names.rs`, between the struct and the test module. A manual visitor (not an untagged enum) so type errors say what was expected instead of "did not match any variant":

```rust
impl<'de> Deserialize<'de> for PypiNames {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PypiNamesVisitor;

        impl<'de> de::Visitor<'de> for PypiNamesVisitor {
            type Value = PypiNames;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a pypi name, a list of pypi names, or null")
            }

            fn visit_str<E: de::Error>(self, name: &str) -> Result<Self::Value, E> {
                Ok(PypiNames(vec![name.to_owned()]))
            }

            fn visit_string<E: de::Error>(self, name: String) -> Result<Self::Value, E> {
                Ok(PypiNames(vec![name]))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(PypiNames(Vec::new()))
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(PypiNames(Vec::new()))
            }

            fn visit_some<D2>(self, deserializer: D2) -> Result<Self::Value, D2::Error>
            where
                D2: Deserializer<'de>,
            {
                deserializer.deserialize_any(PypiNamesVisitor)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let mut names = Vec::with_capacity(seq.size_hint().unwrap_or(1));
                while let Some(name) = seq.next_element::<String>()? {
                    names.push(name);
                }
                Ok(PypiNames(names))
            }
        }

        deserializer.deserialize_any(PypiNamesVisitor)
    }
}

impl Serialize for PypiNames {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p pypi_mapping pypi_names`
Expected: 6 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/pypi_mapping/src/pypi_names.rs crates/pypi_mapping/src/lib.rs
git commit -m "feat: add PypiNames mapping value type accepting one or many names"
```

---

### Task 2: Switch `CompressedMapping` to `PypiNames` values workspace-wide

One atomic type migration: the alias, both resolvers, the manifest field, the TOML value newtype (no list syntax yet — that's Task 3), the `pixi_core` conversion, and the integration tests, including the new multi-purl test that motivates the change. The workspace compiles only at the end of the task, so it is a single commit.

**Files:**
- Modify: `crates/pypi_mapping/src/lib.rs:61-63`
- Modify: `crates/pypi_mapping/src/resolvers/prefix_compressed_resolver.rs:133-149`
- Modify: `crates/pypi_mapping/src/resolvers/project_defined_mapping.rs:287-306, 313-339`
- Modify: `crates/pixi_manifest/src/workspace.rs:360-368`
- Modify: `crates/pixi_manifest/src/toml/conda_pypi_map.rs:59-67, 130-148, 220-234`
- Modify: `crates/pixi_core/src/workspace/conda_pypi_map.rs:16-19, 150-156`
- Modify: `crates/pixi/tests/integration_rust/conda_pypi_map_tests.rs:12-15, 173-175` (+ new test)

- [ ] **Step 1: Flip the alias**

In `crates/pypi_mapping/src/lib.rs` replace lines 61-63:

```rust
/// A compressed mapping is a mapping of a package name to a potential pypi
/// name.
pub type CompressedMapping = HashMap<String, Option<String>>;
```

with:

```rust
/// A compressed mapping maps a conda package name to its PyPI equivalents.
/// An empty [`PypiNames`] means the package is known not to be on PyPI.
pub type CompressedMapping = HashMap<String, PypiNames>;
```

- [ ] **Step 2: Update the prefix.dev compressed resolver**

In `crates/pypi_mapping/src/resolvers/prefix_compressed_resolver.rs` replace lines 133-148:

```rust
        // Determine the mapping for the record
        let Some(potential_pypi_name) = mapping.get(record.package_record.name.as_normalized())
        else {
            return Ok(DerivationOutcome::NotApplicable);
        };

        // If the mapping is empty, there are no purls.
        let Some(pypi_name) = potential_pypi_name else {
            return Ok(DerivationOutcome::NoPurls);
        };

        // Construct the purl
        Ok(DerivationOutcome::Purls(vec![pypi_purl(
            pypi_name,
            Some(PurlDerivationSource::PrefixCompressedMapping),
        )]))
```

with:

```rust
        // Determine the mapping for the record
        let Some(pypi_names) = mapping.get(record.package_record.name.as_normalized()) else {
            return Ok(DerivationOutcome::NotApplicable);
        };

        // If the mapping is empty, there are no purls.
        if pypi_names.0.is_empty() {
            return Ok(DerivationOutcome::NoPurls);
        }

        // Construct one purl per mapped name
        Ok(DerivationOutcome::Purls(
            pypi_names
                .0
                .iter()
                .map(|name| {
                    pypi_purl(
                        name.clone(),
                        Some(PurlDerivationSource::PrefixCompressedMapping),
                    )
                })
                .collect(),
        ))
```

- [ ] **Step 3: Update the project-defined resolver and its TTL-cache tests**

In `crates/pypi_mapping/src/resolvers/project_defined_mapping.rs` replace the match body of `derive_project_defined_purls` (lines 288-305):

```rust
        // Find the mapping for this particular record
        match project_defined_mapping
            .mapping
            .get(record.package_record.name.as_normalized())
        {
            // The record is in the mapping, and it has a pypi name
            Some(Some(mapped_name)) => Ok(DerivationOutcome::Purls(vec![pypi_purl(
                mapped_name.to_string(),
                Some(PurlDerivationSource::ProjectDefinedMapping),
            )])),
            Some(None) => {
                // The record is in the mapping, but it has no pypi name
                Ok(DerivationOutcome::NoPurls)
            }
            None => {
                // The record is not in the mapping
                Ok(DerivationOutcome::NotApplicable)
            }
        }
```

with:

```rust
        // Find the mapping for this particular record
        match project_defined_mapping
            .mapping
            .get(record.package_record.name.as_normalized())
        {
            // The record is in the mapping with one or more pypi names
            Some(pypi_names) if !pypi_names.0.is_empty() => Ok(DerivationOutcome::Purls(
                pypi_names
                    .0
                    .iter()
                    .map(|name| {
                        pypi_purl(
                            name.clone(),
                            Some(PurlDerivationSource::ProjectDefinedMapping),
                        )
                    })
                    .collect(),
            )),
            // The record is in the mapping, but it has no pypi names
            Some(_) => Ok(DerivationOutcome::NoPurls),
            // The record is not in the mapping
            None => Ok(DerivationOutcome::NotApplicable),
        }
```

In the same file's test module, update `write_cache_with_mtime` (lines 315-330): the import line becomes

```rust
    use super::{read_ttl_cache, write_ttl_cache};
    use crate::PypiNames;
```

and the seeded mapping becomes

```rust
        write_ttl_cache(
            &path,
            &[("foo".to_string(), PypiNames(vec!["bar".to_string()]))]
                .into_iter()
                .collect(),
        );
```

In `test_read_ttl_cache_reports_age` (line 337) the assertion becomes:

```rust
        assert_eq!(mapping["foo"], PypiNames(vec!["bar".to_string()]));
```

- [ ] **Step 4: Verify `pypi_mapping` is green**

Run: `cargo test -p pypi_mapping`
Expected: PASS (pypi_names tests + ttl cache tests).

- [ ] **Step 5: Update the manifest types**

In `crates/pixi_manifest/src/workspace.rs` replace lines 364-366:

```rust
    /// Inline conda-name to pypi-name entries. A `None` value (spelled
    /// `false` in TOML) means the package is not a PyPI package.
    pub mapping: Option<HashMap<String, Option<String>>>,
```

with:

```rust
    /// Inline conda-name to pypi-name entries. One conda package may map to
    /// several PyPI names. An empty list (spelled `false` in TOML) means the
    /// package is not a PyPI package.
    pub mapping: Option<HashMap<String, Vec<String>>>,
```

In `crates/pixi_manifest/src/toml/conda_pypi_map.rs`:

Replace the `mapping` extraction (lines 60-67) — only the type annotation changes:

```rust
                let mapping: Option<HashMap<String, Vec<String>>> = th
                    .optional::<TomlHashMap<String, TomlCondaPypiMapValue>>("mapping")
                    .map(|map| {
                        map.into_inner()
                            .into_iter()
                            .map(|(name, value)| (name, value.0))
                            .collect()
                    });
```

Replace `TomlCondaPypiMapValue` (lines 130-148) — list syntax comes in Task 3, this step only normalizes the existing forms to `Vec<String>`:

```rust
/// The value of an inline mapping entry: a pypi name, or `false` to mark the
/// package as not available on PyPI (normalized to an empty list).
pub(crate) struct TomlCondaPypiMapValue(pub(crate) Vec<String>);

impl<'de> toml_span::Deserialize<'de> for TomlCondaPypiMapValue {
    fn deserialize(value: &mut Value<'de>) -> Result<Self, DeserError> {
        match value.take() {
            ValueInner::String(s) => Ok(Self(vec![s.into_owned()])),
            ValueInner::Boolean(false) => Ok(Self(Vec::new())),
            ValueInner::Boolean(true) => Err(custom_error(
                "`true` is not supported; use a string to map the package to a PyPI name, \
                 or `false` to mark it as not a PyPI package",
                value.span,
            )
            .into()),
            other => Err(expected("a string or `false`", other, value.span).into()),
        }
    }
}
```

In the test `test_inline_mapping_with_false_value` (lines 232-233) the assertions become:

```rust
        assert_eq!(mapping["pytorch"], vec!["torch".to_string()]);
        assert_eq!(mapping["not-on-pypi"], Vec::<String>::new());
```

- [ ] **Step 6: Verify `pixi_manifest` is green**

Run: `cargo test -p pixi_manifest conda_pypi_map`
Expected: PASS, no snapshot changes (error messages are unchanged in this task).

- [ ] **Step 7: Update the `pixi_core` conversion**

In `crates/pixi_core/src/workspace/conda_pypi_map.rs` add `PypiNames` to the `pypi_mapping` import (lines 16-19):

```rust
use pypi_mapping::{
    ChannelName, MappingMode, ProjectDefinedChannelMapping, ProjectDefinedMapping,
    ProjectDefinedMappingLocation, PurlDerivationMode, PypiNames,
};
```

and replace the inline-source construction (lines 150-156):

```rust
            if let Some(inline) = mapping {
                sources.push(ProjectDefinedMappingLocation::InMemory(
                    inline
                        .iter()
                        .map(|(name, pypi_name)| (name.to_lowercase(), pypi_name.clone()))
                        .collect(),
                ));
            }
```

with:

```rust
            if let Some(inline) = mapping {
                sources.push(ProjectDefinedMappingLocation::InMemory(
                    inline
                        .iter()
                        .map(|(name, pypi_names)| {
                            (name.to_lowercase(), PypiNames(pypi_names.clone()))
                        })
                        .collect(),
                ));
            }
```

- [ ] **Step 8: Update the integration tests and add the multi-purl test**

In `crates/pixi/tests/integration_rust/conda_pypi_map_tests.rs`:

Add `PypiNames` to the import (lines 12-15):

```rust
use pypi_mapping::{
    self, ProjectDefinedChannelMapping, ProjectDefinedMapping, ProjectDefinedMappingLocation,
    PurlDerivationMode, PurlDerivationSource, PypiNames,
};
```

Replace line 174-175:

```rust
    let compressed_mapping =
        HashMap::from([("foo-bar-car".to_owned(), Some("my-test-name".to_owned()))]);
```

with:

```rust
    let compressed_mapping = HashMap::from([(
        "foo-bar-car".to_owned(),
        PypiNames(vec!["my-test-name".to_owned()]),
    )]);
```

Add the new behavior test directly after `test_purl_are_generated_using_custom_mapping` (after line 209). Note: `purls` is a `BTreeSet`, so the assertion uses sorted order, which matches here:

```rust
#[tokio::test]
async fn test_multiple_pypi_names_generate_multiple_purls() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.workspace().unwrap();
    let client = project.authenticated_client().unwrap();
    let package = Package::build("ambertools", "2").finish();

    let mut repo_data_record = RepoDataRecord {
        identifier: package.identifier(),
        package_record: package.package_record,
        url: Url::parse("https://conda.anaconda.org/conda-forge/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    // One conda package providing several PyPI distributions, the
    // parselmouth `files/v0` list format.
    let compressed_mapping = HashMap::from([(
        "ambertools".to_owned(),
        PypiNames(vec!["parmed".to_owned(), "pytraj".to_owned()]),
    )]);
    let source = HashMap::from([(
        "https://conda.anaconda.org/conda-forge".to_owned(),
        ProjectDefinedChannelMapping::replace(ProjectDefinedMappingLocation::InMemory(
            compressed_mapping,
        )),
    )]);

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    mapping_client
        .amend_purls(
            &PurlDerivationMode::ProjectDefined(Arc::new(ProjectDefinedMapping::new(source))),
            vec![&mut repo_data_record],
            None,
        )
        .await
        .unwrap();

    let purl_names: Vec<&str> = repo_data_record
        .package_record
        .purls
        .as_ref()
        .unwrap()
        .iter()
        .map(|purl| purl.name())
        .collect();
    assert_eq!(purl_names, ["parmed", "pytraj"]);
    assert!(
        repo_data_record
            .package_record
            .purls
            .as_ref()
            .unwrap()
            .iter()
            .all(|purl| purl.to_string().contains("source=project-defined-mapping"))
    );
}
```

- [ ] **Step 9: Build the workspace and run the affected tests**

Run: `cargo build -p pixi_core && cargo test -p pixi --test integration_rust test_multiple_pypi_names_generate_multiple_purls -- --exact`
Expected: build succeeds; the new test PASSES. Then run the surrounding suite:

Run: `cargo test -p pixi --test integration_rust conda_pypi_map`
Expected: PASS (online-gated tests report `ignored` without the `online_tests` feature — that is normal).

If any other workspace crate fails to compile because it constructs `CompressedMapping` values, the fix is mechanical: wrap the value in `PypiNames(vec![...])` (or `PypiNames(Vec::new())` for `None`).

- [ ] **Step 10: Commit**

```bash
git add -A crates
git commit -m "feat: conda-pypi-map values carry one or more pypi names"
```

---

### Task 3: List syntax in inline TOML mappings

`mapping = { airflow = ["airflow", "apache-airflow"] }` becomes valid; the `true` error message learns about lists; `[]` means "not on PyPI".

**Files:**
- Modify: `crates/pixi_manifest/src/toml/conda_pypi_map.rs` (`TomlCondaPypiMapValue` + tests)
- Modify: snapshot files under `crates/pixi_manifest/src/toml/snapshots/` (via insta)

- [ ] **Step 1: Write the failing tests**

Add to the test module of `crates/pixi_manifest/src/toml/conda_pypi_map.rs`, after `test_inline_mapping_with_false_value`:

```rust
    #[test]
    fn test_inline_mapping_with_list_value() {
        let map = parse_map(
            r#"{ conda-forge = { mapping = { airflow = ["airflow", "apache-airflow"] } } }"#,
        );
        let CondaPypiMapEntry::Map(CondaPypiMapSpec { mapping, .. }) =
            get_entry(&map, "conda-forge")
        else {
            panic!("expected a mapping entry");
        };
        let mapping = mapping.expect("mapping should be set");
        assert_eq!(
            mapping["airflow"],
            vec!["airflow".to_string(), "apache-airflow".to_string()]
        );
    }

    #[test]
    fn test_inline_mapping_empty_list_means_not_on_pypi() {
        let map = parse_map(r#"{ conda-forge = { mapping = { not-on-pypi = [] } } }"#);
        let CondaPypiMapEntry::Map(CondaPypiMapSpec { mapping, .. }) =
            get_entry(&map, "conda-forge")
        else {
            panic!("expected a mapping entry");
        };
        assert_eq!(
            mapping.expect("mapping should be set")["not-on-pypi"],
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_inline_list_with_non_string_fails() {
        assert_snapshot!(expect_parse_failure(
            r#"
            [workspace]
            channels = []
            platforms = []
            conda-pypi-map = { conda-forge = { mapping = { pytorch = ["torch", 1] } } }
            "#
        ));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p pixi_manifest conda_pypi_map`
Expected: `test_inline_mapping_with_list_value` and `test_inline_mapping_empty_list_means_not_on_pypi` FAIL (arrays are rejected); `test_inline_list_with_non_string_fails` produces a new snapshot to review.

- [ ] **Step 3: Implement the list arm**

In `TomlCondaPypiMapValue::deserialize`, add an `Array` arm and update the messages. The full match becomes:

```rust
        match value.take() {
            ValueInner::String(s) => Ok(Self(vec![s.into_owned()])),
            ValueInner::Array(items) => {
                let mut names = Vec::with_capacity(items.len());
                for mut item in items {
                    match item.take() {
                        ValueInner::String(s) => names.push(s.into_owned()),
                        other => return Err(expected("a string", other, item.span).into()),
                    }
                }
                Ok(Self(names))
            }
            ValueInner::Boolean(false) => Ok(Self(Vec::new())),
            ValueInner::Boolean(true) => Err(custom_error(
                "`true` is not supported; use a string or a list of strings to map the \
                 package to PyPI name(s), or `false` to mark it as not a PyPI package",
                value.span,
            )
            .into()),
            other => Err(expected(
                "a string, a list of strings or `false`",
                other,
                value.span,
            )
            .into()),
        }
```

Also update the doc comment on the struct:

```rust
/// The value of an inline mapping entry: a pypi name, a list of pypi names,
/// or `false` to mark the package as not available on PyPI (normalized to an
/// empty list).
```

- [ ] **Step 4: Run the tests, review and accept snapshots**

Run: `cargo test -p pixi_manifest conda_pypi_map`
Expected: the two behavior tests PASS; `test_inline_true_value_fails` FAILS with a changed snapshot (message now mentions lists) and `test_inline_list_with_non_string_fails` has a pending new snapshot.

Review the pending snapshots (the `true` message must mention "a list of strings", the non-string-element error must point at the `1` with "expected a string"), then accept:

Run: `cargo insta test --accept -p pixi_manifest` (fallback: `INSTA_UPDATE=always cargo test -p pixi_manifest && cargo test -p pixi_manifest`)
Expected: all `pixi_manifest` tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/pixi_manifest
git commit -m "feat: accept a list of pypi names in inline conda-pypi-map entries"
```

---

### Task 4: HTML-page hint on JSON parse failure

The error that started all this: fetching a GitHub `blob/` URL returns an HTML page and the parse failure is opaque. Extract body parsing into a testable helper that detects HTML and adds a help line.

**Files:**
- Modify: `crates/pypi_mapping/src/resolvers/project_defined_mapping.rs:103-133` (+ tests)

- [ ] **Step 1: Write the failing tests**

Add to the test module of `project_defined_mapping.rs` (extend the existing `use super::...` import with `parse_mapping_body`):

```rust
    #[test]
    fn test_parse_mapping_body_html_gets_raw_url_hint() {
        let err = parse_mapping_body(
            "<!DOCTYPE html><html></html>",
            "https://github.com/org/repo/blob/main/mapping.json",
        )
        .unwrap_err();
        let help = err.help().expect("should carry a help text").to_string();
        assert!(help.contains("raw.githubusercontent.com"), "{help}");
    }

    #[test]
    fn test_parse_mapping_body_plain_json_error_has_no_html_hint() {
        let err = parse_mapping_body("not json", "https://example.com/m.json").unwrap_err();
        assert!(err.help().is_none());
        assert!(err.to_string().contains("https://example.com/m.json"));
    }

    #[test]
    fn test_parse_mapping_body_accepts_all_value_forms() {
        let mapping =
            parse_mapping_body(r#"{"a": "b", "c": ["d", "e"], "f": null}"#, "test").unwrap();
        assert_eq!(mapping["c"], crate::PypiNames(vec!["d".to_string(), "e".to_string()]));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p pypi_mapping parse_mapping_body`
Expected: compile error — `parse_mapping_body` does not exist.

- [ ] **Step 3: Implement the helper and use it in the URL fetch path**

In `fetch_mapping_from_url`, replace the tail (lines 128-132):

```rust
    let mapping_by_name = response.json().await.into_diagnostic().context(format!(
        "failed to parse pypi name mapping located at {url}. Please make sure that it's a valid json"
    ))?;

    Ok(mapping_by_name)
```

with:

```rust
    let body = response.text().await.into_diagnostic().wrap_err(miette::diagnostic!(
        help = LOCATION_FETCH_HELP,
        "failed to download conda-pypi mapping from {}",
        url.as_str()
    ))?;

    parse_mapping_body(&body, url.as_str())
```

Add the helper below `fetch_mapping_from_url`:

```rust
/// Parse a fetched mapping document. An HTML response (e.g. a GitHub `blob/`
/// page URL instead of the raw file) gets an explicit hint, because the bare
/// serde error ("expected value at line 1 column 1") does not tell the user
/// what went wrong.
fn parse_mapping_body(body: &str, source: &str) -> miette::Result<CompressedMapping> {
    serde_json::from_str(body).map_err(|err| {
        if body.trim_start().starts_with('<') {
            miette::miette!(
                help = "the response looks like an HTML page, not JSON. If this is a GitHub \
                        link, use the raw file URL (raw.githubusercontent.com) instead of the \
                        `blob/` page.",
                "failed to parse pypi name mapping located at {source}. Please make sure that \
                 it's a valid json: {err}"
            )
        } else {
            miette::miette!(
                "failed to parse pypi name mapping located at {source}. Please make sure that \
                 it's a valid json: {err}"
            )
        }
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p pypi_mapping`
Expected: all PASS, including the three new tests.

- [ ] **Step 5: Commit**

```bash
git add crates/pypi_mapping
git commit -m "feat: hint at raw URLs when a fetched conda-pypi mapping is an HTML page"
```

---

### Task 5: JSON schema

**Files:**
- Modify: `schema/model.py:209-212`
- Modify: `schema/examples/valid/full.toml:7`
- Regenerate: `schema/schema.json`, `schema/pyproject/schema.json`, `schema/pyproject/partial-pixi.json`

- [ ] **Step 1: Update the model and the example**

In `schema/model.py` replace lines 209-212:

```python
    mapping: dict[NonEmptyStr, NonEmptyStr | Literal[False]] | None = Field(
        None,
        description="Inline `conda_name: pypi_name` entries; `false` marks a package as not available on PyPI. Inline entries override entries from `location`.",
    )
```

with:

```python
    mapping: dict[NonEmptyStr, NonEmptyStr | list[NonEmptyStr] | Literal[False]] | None = Field(
        None,
        description="Inline `conda_name: pypi_name` entries; a list maps one conda package to several PyPI names, `false` marks a package as not available on PyPI. Inline entries override entries from `location`.",
    )
```

In `schema/examples/valid/full.toml` line 7, inside the `"pytorch"` entry's mapping table, add a list-valued entry so the schema test covers it. The entry `mapping = { pytorch = "torch", not-on-pypi = false }` becomes:

```toml
mapping = { pytorch = "torch", airflow = ["airflow", "apache-airflow"], not-on-pypi = false }
```

- [ ] **Step 2: Regenerate and test the schema**

Run: `pixi run test-schema` (depends on `generate-schema`, so this regenerates all three JSON files first)
Expected: schema files regenerated with the `array` alternative under the `mapping` additionalProperties; pytest PASSES.

- [ ] **Step 3: Commit**

```bash
git add schema
git commit -m "feat: allow lists of pypi names in the conda-pypi-map json schema"
```

---

### Task 6: Documentation

**Files:**
- Modify: `docs/reference/pixi_manifest.md:202-232`

- [ ] **Step 1: Update the `conda-pypi-map` reference section**

Three edits in `docs/reference/pixi_manifest.md`:

1. In the TOML example (line 208), extend the inline mapping:

```toml
# Inline entries, no file needed. A list maps one conda package to several
# PyPI packages; `false` means "not a PyPI package".
conda-forge = { mode = "extend", mapping = { pytorch = "torch", airflow = ["airflow", "apache-airflow"], not-on-pypi = false } }
```

2. Replace the mapping-file paragraph and JSON example (lines 217-225):

```markdown
Mapping files are structured in `json` format with `conda_name: pypi_package_name` entries.
The value can also be a list of PyPI names — the conda package then satisfies all of them and one purl is emitted per name — or `null` to mark a package as not available on PyPI.
This is the same format parselmouth publishes under [`files/v0/<channel>/compressed_mapping.json`](https://github.com/prefix-dev/parselmouth/tree/main/files/v0), so those files can be used directly (use the raw file URL).

```json title="local/robostack_mapping.json"
{
  "jupyter-ros": "my-name-from-mapping",
  "airflow": ["airflow", "apache-airflow"],
  "boltons": "boltons-pypi",
  "not-on-pypi": null
}
```​
```

3. Update the `mapping` bullet (line 230):

```markdown
- `mapping`: inline `conda_name = "pypi_name"` entries. A list of names maps one conda package to several PyPI packages; a value of `false` marks the package as not available on PyPI. Inline entries override entries from `location`.
```

- [ ] **Step 2: Build the docs page (optional sanity check) and commit**

If a docs preview task exists it is not required for this change; a markdown review is enough.

```bash
git add docs/reference/pixi_manifest.md
git commit -m "docs: document multi-name conda-pypi-map values"
```

---

### Task 7: Fix the example that triggered the bug

**Files:**
- Modify: `examples/conda-pypi-map/b-additive-flip/pixi.toml:8`
- Modify: `examples/conda-pypi-map/README.md` (section `b-additive-flip`)

- [ ] **Step 1: Point the example at the raw v0 file**

In `examples/conda-pypi-map/b-additive-flip/pixi.toml` replace line 8:

```toml
conda-forge = "https://github.com/prefix-dev/parselmouth/blob/main/files/v0/conda-forge/compressed_mapping.json"
```

with:

```toml
conda-forge = "https://raw.githubusercontent.com/prefix-dev/parselmouth/main/files/v0/conda-forge/compressed_mapping.json"
```

In `examples/conda-pypi-map/README.md`, in the `## b-additive-flip` section, after the existing code block add:

```markdown
The mapping is parselmouth's `files/v0` document, whose values are *lists*
of pypi names (`"airflow": ["airflow", "apache-airflow"]`) — this scenario
also covers the multi-name format. Pointing at the `github.com/...//blob/`
page instead of the raw URL fails with a hint to use
`raw.githubusercontent.com`.
```

- [ ] **Step 2: Manually verify (requires network)**

```bash
cargo build --bin pixi
cd examples/conda-pypi-map/b-additive-flip
rm -f pixi.lock && ../../../target/debug/pixi lock
grep -B4 'pkg:pypi/boltons' pixi.lock
cd ../../..
```

Expected: lock succeeds (previously: "failed to parse pypi name mapping"); boltons keeps its purl. Also verify the new hint by temporarily putting the `blob/` URL back and re-locking — the error must mention `raw.githubusercontent.com`. Restore the raw URL afterwards and `rm -f pixi.lock` (lock files in these scenario dirs are not committed).

- [ ] **Step 3: Commit**

```bash
git add examples/conda-pypi-map
git commit -m "fix: use the raw parselmouth v0 url in the conda-pypi-map example"
```

---

### Task 8: Final verification

- [ ] **Step 1: Format, lint, test**

```bash
cargo fmt --all
cargo clippy -p pypi_mapping -p pixi_manifest -p pixi_core --all-targets -- -D warnings
cargo test -p pypi_mapping -p pixi_manifest
cargo test -p pixi --test integration_rust conda_pypi_map
```

Expected: no formatting diffs, no clippy warnings, all tests PASS (online-gated tests ignored).

- [ ] **Step 2: Commit any formatting fallout**

```bash
git add -A && git diff --cached --quiet || git commit -m "fix: formatting"
```
