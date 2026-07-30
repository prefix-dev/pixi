#!/usr/bin/env bash
#
# Compare pixi's SHA-256 throughput with and without the hardware-accelerated
# backend. Most interesting on aarch64 (Apple silicon, ARM Linux), where the
# `sha2` 0.10 that the vendored `uv` crates use only reaches for the ARMv8 SHA-2
# instructions when its `asm` feature is enabled.
#
# The portable software backend is measured first and saved as the `soft`
# baseline, then the default (accelerated) build is measured against it, so the
# second run reports the speedup per benchmark.
#
# Usage:
#   ./scripts/bench-sha256.sh              # run both configurations
#   ./scripts/bench-sha256.sh --quick      # same, but with a shorter sampling
#                                          # time (any extra arguments are
#                                          # forwarded to criterion)

set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> Baseline: portable software backend"
cargo bench -p pixi_bench --bench sha256 --features force-soft -- \
    --save-baseline soft "$@"

echo
echo "==> Default: hardware-accelerated backend, compared against 'soft'"
cargo bench -p pixi_bench --bench sha256 -- \
    --baseline soft "$@"
