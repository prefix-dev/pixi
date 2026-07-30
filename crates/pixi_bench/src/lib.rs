//! Micro-benchmarks for hot paths in pixi.
//!
//! There is no library code here, the crate exists to host the benchmarks under
//! `benches/`. Run them with:
//!
//! ```shell
//! cargo bench -p pixi_bench
//! ```
//!
//! To compare the hardware-accelerated SHA-256 backend against the portable
//! software one, use `scripts/bench-sha256.sh`.
