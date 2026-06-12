# Multi-name values in `conda-pypi-map`

**Date:** 2026-06-12
**Status:** Approved
**Branch:** feat-additive-conda-pypi-map (extends PR #6333)

## Problem

A project-defined `conda-pypi-map` location pointing at parselmouth's
`files/v0/<channel>/compressed_mapping.json` fails to parse. Two formats
exist in the wild:

| Source | Value format |
|---|---|
| `conda-mapping.prefix.dev/compressed-v0/compressed_mapping.json` (built-in resolver) | `"conda": "pypi"` or `null` |
| parselmouth `files/v0/<channel>/compressed_mapping.json` | `"conda": ["pypi", ...]` or `null` |

Pixi's `CompressedMapping = HashMap<String, Option<String>>` only accepts
the first. The real v0 file (33,143 entries) has 12,978 nulls and 206
multi-name entries (e.g. `airflow → ["airflow", "apache-airflow"]`,
`ambertools` → 11 names) and no empty arrays.

A secondary papercut triggered the report: a `github.com/.../blob/...`
URL returns the GitHub HTML page, producing an opaque "error decoding
response body" instead of pointing the user at the raw URL.

## Decisions

- Accept the array form everywhere a user writes a mapping: JSON files
  fetched via `location` (URL or path) **and** the inline TOML
  `mapping = {...}` table. Single-string, array, and `null`/`false`
  values may be mixed freely in one document.
- `[]` is equivalent to `false` (TOML) / `null` (JSON): the package has
  no PyPI equivalent and gets `purls: []`.
- A conda package mapping to N names emits N purls, in file order.
- Out of scope: the reverse `pypi-conda-map` in pixi-build-python stays
  single-name; the built-in prefix.dev compressed resolver endpoint is
  untouched.

## Design

### 1. Data model

One core type change in `crates/pypi_mapping/src/lib.rs`:

```rust
// before
pub type CompressedMapping = HashMap<String, Option<String>>;
// after
pub type CompressedMapping = HashMap<String, PypiNames>;

/// The PyPI equivalents of one conda package. Empty means "not on PyPI".
pub struct PypiNames(pub Vec<String>);
```

`PypiNames` is a newtype so it can carry custom serde:

- **Deserialize** accepts a string (→ 1-element vec), an array of
  strings, or `null` (→ empty vec).
- **Serialize** always writes an array (canonical form).

Absent key / empty vec / non-empty vec encode the three derivation
outcomes (`NotApplicable` / `NoPurls` / `Purls`).

Downstream holders of `Option<String>` values follow mechanically:
`CondaPypiMapSpec.mapping` (`pixi_manifest/src/workspace.rs`) becomes
`HashMap<String, Vec<String>>`; the `InMemory` conversion
(`pixi_core/src/workspace/conda_pypi_map.rs`) wraps values in
`PypiNames`.

### 2. Parsing — JSON locations

No code change beyond the type: `response.json()`,
`serde_json::from_reader`, and the TTL-cache `from_str` all route
through `PypiNames::deserialize`, so old-format, new-format, and mixed
files parse.

**HTML hint:** in
`crates/pypi_mapping/src/resolvers/project_defined_mapping.rs`, when the
fetched body fails JSON parsing and starts with `<`, the error gains a
help line: "the response looks like an HTML page, not JSON; if this is a
GitHub link, use the raw.githubusercontent.com URL".

### 3. Parsing — inline TOML

`TomlCondaPypiMapValue` (`pixi_manifest/src/toml/conda_pypi_map.rs`)
gains an `Array` arm; every element must be a string, anything else is a
span error. Valid forms:

```toml
[workspace.conda-pypi-map]
conda-forge = { mapping = { pytorch = "torch", airflow = ["airflow", "apache-airflow"], not-on-pypi = false } }
```

`true` stays rejected; its error message is extended to mention lists.

### 4. Derivation

`derive_project_defined_purls`
(`crates/pypi_mapping/src/resolvers/project_defined_mapping.rs`) maps
each name to a purl, preserving order: non-empty →
`Purls(names.map(pypi_purl))`, empty → `NoPurls`, absent →
`NotApplicable`.

### 5. Cache & merge compatibility

- Old TTL-cache files (string values) parse under the lenient
  deserializer. New cache files are written as arrays; an *older* pixi
  reading one fails to parse, which `read_ttl_cache` already treats as
  "no cache" → refetch. Graceful in both directions.
- Merge semantics unchanged: later sources replace earlier entries
  wholesale per key (`["a", "b"]` replaces an earlier `"a"`, never
  appends).
- Side effect: the built-in prefix.dev compressed resolver shares
  `CompressedMapping` and silently tolerates arrays too. Harmless — its
  endpoint is string-format and not user-configurable.

### 6. Testing

- **TOML:** parse tests for array values, mixed table, empty array
  (→ no-purl), array containing a non-string (error), updated snapshot
  for the `true` error message.
- **JSON:** unit tests parsing old-format, new-format, and mixed
  documents; TTL-cache round-trip with multi-name values (existing
  tests update mechanically).
- **Derivation:** a multi-name entry yields multiple purls in order;
  integration test in `conda_pypi_map_tests.rs` asserting a lock with
  two `pkg:pypi/...?source=project-defined-mapping` purls on one conda
  package.
- **Manual:** fix the `examples/conda-pypi-map/b-additive-flip` example
  to the `raw.githubusercontent.com` URL; README step asserts the real
  v0 file locks (and optionally a multi-purl package like
  `ambertools`).

### 7. Docs

The `conda-pypi-map` reference docs gain the array form and a sentence
on multi-purl semantics, and mention that parselmouth's `files/v0/...`
format is directly usable.
