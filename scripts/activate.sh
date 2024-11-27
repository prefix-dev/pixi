#!/bin/bash
set -Eeuo pipefail
export CARGO_TARGET_DIR="target/pixi"

# on linux we need to set these rust flags
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="clang"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=$CONDA_PREFIX/bin/mold"

# on macOS we need to set these rust flags
export CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-Wl,-rpath,$CONDA_PREFIX/lib"
export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-Wl,-rpath,$CONDA_PREFIX/lib"