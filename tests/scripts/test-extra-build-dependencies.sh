#!/usr/bin/env bash

# Run from repository root with:
#   bash tests/scripts/test-extra-build-dependencies.sh
set -euo pipefail

echo "Running extra-build-dependencies scoped/merged regression tests"

cargo test -p pixi_manifest test_parse_extra_build_dependencies -- --nocapture
cargo test -p pixi_manifest test_parse_feature_scoped_extra_build_dependencies -- --nocapture
cargo test -p pixi_manifest test_parse_invalid_feature_scoped_extra_build_dependencies -- --nocapture
cargo test -p pixi_core test_extra_build_dependencies_are_scoped_and_merged_per_environment -- --nocapture
cargo test -p pixi_core test_extra_build_dependencies_respect_no_default_feature -- --nocapture
cargo test -p pixi_core test_extra_build_dependencies_merge_order_includes_default_last -- --nocapture
cargo test -p pixi_core test_extra_build_dependencies_empty_tables_are_ignored -- --nocapture
cargo test -p pixi_uv_conversions empty_extra_build_dependencies_is_noop -- --nocapture
cargo test -p pixi_uv_conversions empty_some_extra_build_dependencies_is_noop -- --nocapture
cargo test -p pixi_uv_conversions converts_multiple_extra_build_dependencies_for_multiple_packages -- --nocapture
cargo test -p pixi --test integration_rust test_extra_build_dependencies_manifest_key_with_wheel_install -- --nocapture
cargo test -p pixi --test integration_rust test_extra_build_dependencies_feature_scoped_with_multiple_environments -- --nocapture

echo "extra-build-dependencies regression tests passed"
