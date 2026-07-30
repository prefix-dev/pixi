//! SHA-256 throughput.
//!
//! Every artifact pixi installs gets hashed: conda packages are verified against
//! the digest in the lock file, and wheels are hashed both for verification and
//! to key uv's unzipped-wheel cache. On a large environment that adds up to
//! gigabytes of hashing, so the backend `sha2` picks matters.
//!
//! Two backends are exercised, because pixi ends up with two copies of `sha2` in
//! its dependency graph:
//!
//! - `sha2` 0.10, used by the vendored `uv` crates for PyPI wheels. On aarch64
//!   this only uses the ARMv8 SHA-2 instructions when the `asm` feature is on.
//! - `sha2` 0.11 behind `rattler_digest`, used for conda packages. It detects
//!   the instructions at runtime with no feature flag needed.
//!
//! Compare the accelerated and portable backends with:
//!
//! ```shell
//! ./scripts/bench-sha256.sh
//! ```

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group};

/// Payload sizes bracketing what pixi hashes in practice, from a small
/// pure-Python wheel up to a chunky binary wheel or conda package.
const SIZES: &[(&str, usize)] = &[
    ("64KiB", 64 * 1024),
    ("1MiB", 1024 * 1024),
    ("10MiB", 10 * 1024 * 1024),
];

/// Hashing happens while bytes stream off the network or off disk, never in one
/// shot, so measure that pattern too.
const CHUNK: usize = 64 * 1024;

/// Deterministic, incompressible-looking filler. An xorshift keeps generation
/// cheap so setup does not dominate the larger sizes.
fn payload(len: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn hash_uv(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

fn hash_conda(data: &[u8]) -> [u8; 32] {
    use rattler_digest::digest::Digest;

    let mut hasher = rattler_digest::Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

fn hash_uv_chunked(data: &[u8]) -> [u8; 32] {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    for chunk in data.chunks(CHUNK) {
        hasher.update(chunk);
    }
    hasher.finalize().into()
}

fn hash_conda_chunked(data: &[u8]) -> [u8; 32] {
    use rattler_digest::digest::Digest;

    let mut hasher = rattler_digest::Sha256::new();
    for chunk in data.chunks(CHUNK) {
        hasher.update(chunk);
    }
    hasher.finalize().into()
}

fn one_shot(c: &mut Criterion) {
    let mut group = c.benchmark_group("sha256/one-shot");

    for (label, size) in SIZES {
        let data = payload(*size);
        group.throughput(Throughput::Bytes(*size as u64));

        group.bench_with_input(BenchmarkId::new("uv-sha2-0.10", label), &data, |b, data| {
            b.iter(|| black_box(hash_uv(black_box(data))));
        });
        group.bench_with_input(
            BenchmarkId::new("conda-sha2-0.11", label),
            &data,
            |b, data| {
                b.iter(|| black_box(hash_conda(black_box(data))));
            },
        );
    }

    group.finish();
}

fn streaming(c: &mut Criterion) {
    let mut group = c.benchmark_group("sha256/streamed-64KiB-chunks");

    // Only the larger sizes are interesting here, a 64 KiB payload is a single
    // chunk and would just repeat the one-shot numbers.
    for (label, size) in SIZES.iter().filter(|(_, size)| *size > CHUNK) {
        let data = payload(*size);
        group.throughput(Throughput::Bytes(*size as u64));

        group.bench_with_input(BenchmarkId::new("uv-sha2-0.10", label), &data, |b, data| {
            b.iter(|| black_box(hash_uv_chunked(black_box(data))));
        });
        group.bench_with_input(
            BenchmarkId::new("conda-sha2-0.11", label),
            &data,
            |b, data| {
                b.iter(|| black_box(hash_conda_chunked(black_box(data))));
            },
        );
    }

    group.finish();
}

/// Which backend `sha2` 0.10 ends up on depends on the target, the enabled
/// features *and* the CPU, which is easy to get wrong. State it up front rather
/// than leaving it to be inferred from the numbers.
fn uv_backend() -> &'static str {
    #[cfg(feature = "force-soft")]
    {
        "portable software (forced via the `force-soft` feature)"
    }

    #[cfg(all(not(feature = "force-soft"), target_arch = "aarch64", not(windows)))]
    {
        // The `asm` feature still dispatches at runtime through `cpufeatures`.
        if std::arch::is_aarch64_feature_detected!("sha2") {
            "ARMv8 SHA-2 instructions"
        } else {
            "portable software (this CPU has no ARMv8 SHA-2 support)"
        }
    }

    #[cfg(all(
        not(feature = "force-soft"),
        any(target_arch = "x86", target_arch = "x86_64")
    ))]
    {
        "runtime-detected (x86 SHA-NI when the CPU has it)"
    }

    #[cfg(all(
        not(feature = "force-soft"),
        not(all(target_arch = "aarch64", not(windows))),
        not(any(target_arch = "x86", target_arch = "x86_64"))
    ))]
    {
        "portable software (no accelerated backend for this target)"
    }
}

/// A backend that dispatches to the wrong compression function would still
/// produce numbers, just meaningless ones, so check both against a known digest
/// before measuring anything.
fn verify_backends() {
    // SHA-256 of "abc", from FIPS 180-4.
    const ABC: [u8; 32] = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22,
        0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00,
        0x15, 0xad,
    ];

    assert_eq!(hash_uv(b"abc"), ABC, "uv-sha2-0.10 one-shot is wrong");
    assert_eq!(hash_conda(b"abc"), ABC, "conda-sha2-0.11 one-shot is wrong");

    // Spanning more than one chunk exercises the multi-block path, which is
    // where the hardware backends actually kick in.
    let data = payload(3 * CHUNK + 7);
    assert_eq!(
        hash_uv_chunked(&data),
        hash_uv(&data),
        "uv-sha2-0.10 disagrees with itself across chunk boundaries"
    );
    assert_eq!(
        hash_conda_chunked(&data),
        hash_uv(&data),
        "the two sha2 versions disagree on a multi-block payload"
    );
}

criterion_group!(name = sha256; config = Criterion::default(); targets = one_shot, streaming);

fn main() {
    eprintln!("sha256 backends:");
    eprintln!("  uv-sha2-0.10     {}", uv_backend());
    eprintln!("  conda-sha2-0.11  runtime-detected (hardware when the CPU has it)");

    verify_backends();

    sha256();

    Criterion::default().configure_from_args().final_summary();
}
