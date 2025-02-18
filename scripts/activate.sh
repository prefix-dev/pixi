# Ignore requiring a shebang as this is a script meant to be sourced
# shellcheck disable=SC2148

# Setup the mold linker when targeting x86_64-unknown-linux-gnu
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="clang"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=$CONDA_PREFIX/bin/mold"

# On macOS we need to set these rust flags to avoid the following error:
# dyld[98511]: Library not loaded: @rpath/liblzma.5.dylib
#   Referenced from: <E86679E3-7383-3039-9E4A-031C60A071A5> ..
#   Reason: no LC_RPATH's found
export CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-Wl,-rpath,$CONDA_PREFIX/lib"
export CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS="-C link-arg=-Wl,-rpath,$CONDA_PREFIX/lib"
