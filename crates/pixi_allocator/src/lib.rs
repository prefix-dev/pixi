//! This crate pulls in specific allocators for different platforms to optimize
//! the performance.
//!
//! This is placed in a separate crate to easily allow conditional compilation.
//! Compiling these crates can take a long time and we do not always care about
//! the extra performance.

#[cfg(target_os = "windows")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(all(
    not(target_os = "windows"),
    not(target_os = "openbsd"),
    not(target_os = "freebsd"),
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64"
    )
))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
