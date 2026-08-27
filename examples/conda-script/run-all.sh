#!/usr/bin/env bash
# Runs every conda-script example against the locally built pixi.
#
# Usage: ./run-all.sh [PIXI]
#
# PIXI defaults to the release binary (build it with `pixi run build-release`);
# pass another binary to verify that one instead.
set -euo pipefail

directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
pixi=${1:-"$directory/../../target/pixi/release/pixi"}
if ! command -v -- "$pixi" >/dev/null; then
    printf '%s not found; build it with `pixi run build-release`\n' "$pixi" >&2
    exit 1
fi

failed=()
for script in "$directory"/*/main.*; do
    printf '\n=== %s ===\n' "${script#"$directory"/}"
    if ! "$pixi" run --experimental --script "$script"; then
        failed+=("${script#"$directory"/}")
    fi
done

if [ ${#failed[@]} -gt 0 ]; then
    printf '\nfailed: %s\n' "${failed[*]}"
    exit 1
fi
printf '\nall examples ran\n'
