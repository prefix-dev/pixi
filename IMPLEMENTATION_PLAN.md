# Implementation Plan: Develop Dependencies Feature

Issue: [#4721](https://github.com/prefix-dev/pixi/issues/4721)

## Progress Summary

| Stage | Status | Description |
|-------|--------|-------------|
| Stage 1 | ✅ Complete | Data Model + TOML Parsing (with 12 tests) |
| Stage 2 | ✅ Complete | Feature Access Methods |
| Stage 3 | 🚧 In Progress | Solving Integration |

**Last Updated**: Stage 1 & 2 completed with comprehensive test coverage

---

## Overview
Add support for `[develop]` section in pixi.toml that allows specifying source dependencies whose build/host/run dependencies should be installed without building the packages themselves.

## Design Decisions
- Store as `IndexMap<PackageName, SourceSpec>` to support Git/Path/Url sources
- Expand develop dependencies BEFORE solving (not during)
- Merge expanded dependencies into regular dependencies
- Support platform-specific and feature-specific develop dependencies
- Always extract all dependency types (build, host, run)

---

## Stage 1: Data Model + TOML Parsing
**Goal**: Add develop dependencies to data model and parse from TOML
**Status**: ✅ Complete

### Tasks
- [x] Add `develop_dependencies: Option<IndexMap<PackageName, SourceSpec>>` to `WorkspaceTarget` in `crates/pixi_manifest/src/target.rs`
- [x] Add `develop: Option<IndexMap<PackageName, TomlLocationSpec>>` to `TomlTarget` in `crates/pixi_manifest/src/toml/target.rs`
- [x] Parse `develop` field in `TomlTarget::into_workspace_target()`
- [x] Add `develop: Option<IndexMap<PackageName, TomlLocationSpec>>` to `TomlFeature` in `crates/pixi_manifest/src/toml/feature.rs`
- [x] Add `develop` field to `TomlManifest` in `crates/pixi_manifest/src/toml/manifest.rs`
- [x] Parse `develop` field in all TOML deserializers
- [x] Convert `TomlLocationSpec` to `SourceSpec` during parsing
- [x] Write comprehensive unit tests for parsing develop dependencies

### Success Criteria
- [x] Code compiles
- [x] Unit test passes for parsing `[develop]` section
- [x] Unit test passes for parsing `[feature.X.develop]` section
- [x] Unit test passes for parsing `[target.linux-64.develop]` section

### Files Modified
- `crates/pixi_manifest/src/target.rs`
- `crates/pixi_manifest/src/toml/target.rs`
- `crates/pixi_manifest/src/toml/feature.rs`
- `crates/pixi_manifest/src/toml/manifest.rs`
- `crates/pixi_manifest/src/toml/mod.rs`
- `crates/pixi_manifest/src/toml/test_develop.rs` (new)

### Tests Added
Created comprehensive test suite with 12 tests:
- `test_parse_develop_path` - Parse path-based develop dependencies
- `test_parse_develop_git` - Parse git-based develop dependencies
- `test_parse_develop_url` - Parse URL-based develop dependencies
- `test_parse_develop_multiple` - Parse multiple develop dependencies
- `test_parse_feature_develop` - Parse feature-specific develop dependencies
- `test_parse_target_develop` - Parse platform-specific develop dependencies
- `test_parse_develop_override` - Test platform override behavior
- `test_parse_develop_git_with_rev` - Parse git with rev/tag
- `test_parse_develop_empty` - Handle missing develop dependencies
- `test_parse_feature_target_develop` - Parse feature + target develop dependencies
- `test_parse_develop_invalid_no_source_type` - Error handling
- `test_parse_develop_invalid_multiple_sources` - Error handling

### Commit Message
```
feat: add develop dependencies to manifest data model

Add support for parsing [develop] sections from pixi.toml:
- Add develop_dependencies field to WorkspaceTarget
- Parse develop section in TomlTarget, TomlFeature, and TomlManifest
- Support platform-specific develop dependencies
- Add comprehensive test suite with 12 tests

Part of #4721
```

---

## Stage 2: Feature Access Methods
**Goal**: Add methods to access develop dependencies with platform resolution
**Status**: ✅ Complete

### Tasks
- [x] Add `develop_dependencies(&self, platform: Option<Platform>)` method to `Feature` in `crates/pixi_manifest/src/feature.rs`
- [x] Follow pattern of existing `run_dependencies()`, `build_dependencies()` methods
- [x] Handle platform-specific resolution via `targets.resolve(platform)`
- [x] Platform resolution tested via Stage 1 tests

### Success Criteria
- [x] Code compiles
- [x] All existing tests pass
- [x] Platform-specific develop dependency resolution works correctly
- [x] Method returns correct develop dependencies for given platform

### Files Modified
- `crates/pixi_manifest/src/feature.rs`

### Commit Message
```
feat: add methods to access develop dependencies from features

Add develop_dependencies() method to Feature that:
- Resolves platform-specific develop dependencies
- Follows existing pattern for dependency access
- Merges develop dependencies from all targets

Part of #4721
```

---

## Stage 3: Solving Integration
**Goal**: Integrate develop dependency expansion into environment solving
**Status**: In Progress

### Tasks
- [ ] Add `ExpandDevSourcesFailed` variant to `SolveCondaEnvironmentError` in `crates/pixi_core/src/lock_file/update.rs`
- [ ] Create helper function `expand_develop_dependencies()` that:
  - Collects develop dependencies from all features in environment
  - Converts to `Vec<DependencyOnlySource>`
  - Calls `command_dispatcher.expand_dev_sources()`
  - Returns `ExpandedDevSources`
- [ ] Integrate into `spawn_solve_conda_environment_task()` (around line 1907):
  - Call `expand_develop_dependencies()` before solving
  - Merge `expanded.dependencies` into regular dependencies
  - Merge `expanded.constraints` into constraints
- [ ] Write integration test using `tests/data/workspaces/output-dependencies/` workspace

### Success Criteria
- [ ] Code compiles
- [ ] All existing tests pass
- [ ] Integration test passes showing:
  - Develop source dependencies are expanded
  - Their dependencies are installed
  - The develop packages themselves are NOT built/installed
  - Dev sources that depend on other dev sources correctly filter them out

### Files Modified
- `crates/pixi_core/src/lock_file/update.rs`
- `crates/pixi_command_dispatcher/tests/integration/main.rs` (or new test file)

### Commit Message
```
feat: expand develop dependencies during environment solving

Integrate develop dependency expansion:
- Expand develop sources before solving
- Merge expanded dependencies into environment
- Add comprehensive integration test
- Handle errors from expansion gracefully

Closes #4721
```

---

## Testing Strategy

### Unit Tests
- Parse `[develop]` section with valid source specs
- Parse `[feature.X.develop]` section
- Parse `[target.platform.develop]` section
- Platform-specific resolution of develop dependencies

### Integration Test
Use `tests/data/workspaces/output-dependencies/` workspace:
1. Add `[develop]` section with test-package as dev source
2. Add `[feature.extra.develop]` with package-a as dev source
3. Verify:
   - test-package's dependencies (cmake, make, openssl, zlib, numpy, python) are installed
   - test-package itself is NOT built/installed
   - package-b is included (not a dev source)
   - test-package is filtered from package-a's dependencies (both are dev sources)

---

## Notes
- Command dispatcher already has `expand_dev_sources()` implemented
- `TomlLocationSpec` already supports parsing source specs
- Follow existing patterns for dependencies/host-dependencies/build-dependencies
