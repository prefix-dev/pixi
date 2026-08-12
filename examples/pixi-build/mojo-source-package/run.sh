#!/usr/bin/env bash
set -euo pipefail

example_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$example_dir/../../.." && pwd)"
backend="$repo_root/target/debug/pixi-build-mojo"
pixi_bin="$repo_root/target/debug/pixi"
environment="${1:-default}"

case "$environment" in
  default|source|precompiled) ;;
  *)
    echo "usage: $0 [default|source|precompiled]" >&2
    exit 2
    ;;
esac

cargo build --manifest-path "$repo_root/Cargo.toml" -p pixi -p pixi-build-mojo

PIXI_BUILD_BACKEND_OVERRIDE="pixi-build-mojo=$backend" \
  "$pixi_bin" run --manifest-path "$example_dir/pixi.toml" \
  --environment "$environment" start
