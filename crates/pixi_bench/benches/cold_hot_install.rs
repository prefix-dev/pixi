use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn create_pixi_project(project_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs::File;
    use std::io::Write;

    let pixi_toml = r#"[project]
name = "benchmark-project"
version = "0.1.0"
description = "Benchmark project for pixi add"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

[dependencies]
"#;

    let mut file = File::create(project_dir.join("pixi.toml"))?;
    file.write_all(pixi_toml.as_bytes())?;
    Ok(())
}

fn clear_pixi_cache() -> Result<(), Box<dyn std::error::Error>> {
    println!("Clearing pixi cache...");

    // Clear the global pixi cache
    let output = Command::new("pixi")
        .args(["clean", "cache", "-a"])
        .output()
        .expect("Failed to execute pixi clean cache");

    if !output.status.success() {
        eprintln!(
            "Warning: pixi clean cache failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Also try to clear common cache locations manually
    let home_dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let cache_dirs = [
        format!("{}/.cache/pixi", home_dir),
        format!("{}/.pixi", home_dir),
        "/tmp/pixi-cache".to_string(),
    ];

    for cache_dir in &cache_dirs {
        let path = PathBuf::from(cache_dir);
        if path.exists() {
            println!("Removing cache directory: {:?}", path);
            #[allow(clippy::disallowed_methods)]
            let _ = fs::remove_dir_all(&path);
        }
    }

    println!("Cache cleared");
    Ok(())
}

fn warm_up_cache(packages: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    println!("Warming up cache with packages: {:?}", packages);

    let temp_dir = TempDir::new()?;
    let project_path = temp_dir.path();
    create_pixi_project(project_path)?;

    // Add packages to warm up the cache
    let mut cmd = Command::new("pixi");
    cmd.arg("add").current_dir(project_path);

    for package in packages {
        cmd.arg(package);
    }

    let output = cmd.output()?;

    if !output.status.success() {
        eprintln!(
            "Warning: cache warm-up failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    } else {
        println!("Cache warmed up successfully");
    }

    Ok(())
}

fn pixi_add_packages_timed(packages: &[&str]) -> u64 {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    create_pixi_project(project_path).expect("Failed to create pixi project");

    let mut cmd = Command::new("pixi");
    cmd.arg("add").current_dir(project_path);

    for package in packages {
        cmd.arg(package);
    }

    println!("Timing: pixi add {}", packages.join(" "));

    let start = Instant::now();
    let output = cmd.output().expect("Failed to execute pixi add");
    let duration = start.elapsed();

    if !output.status.success() {
        eprintln!(
            "pixi add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return 0;
    } else {
        println!(
            "Added {} packages in {:.2}s",
            packages.len(),
            duration.as_secs_f64()
        );
    }

    // Return nanoseconds as u64 for Codespeed
    duration.as_nanos() as u64
}

// Cold cache benchmarks - clear cache before each run
fn bench_cold_cache_small(c: &mut Criterion) {
    let packages = ["numpy"];

    c.bench_function("cold_cache_small", |b| {
        b.iter(|| {
            clear_pixi_cache().expect("Failed to clear cache");
            std::thread::sleep(Duration::from_secs(1));

            let duration_ns = pixi_add_packages_timed(&packages);
            black_box(duration_ns)
        })
    });
}

fn bench_cold_cache_medium(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "requests"];

    c.bench_function("cold_cache_medium", |b| {
        b.iter(|| {
            clear_pixi_cache().expect("Failed to clear cache");
            std::thread::sleep(Duration::from_secs(1));

            let duration_ns = pixi_add_packages_timed(&packages);
            black_box(duration_ns)
        })
    });
}

fn bench_cold_cache_large(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "scipy", "matplotlib", "requests"];

    c.bench_function("cold_cache_large", |b| {
        b.iter(|| {
            clear_pixi_cache().expect("Failed to clear cache");
            std::thread::sleep(Duration::from_secs(2));

            let duration_ns = pixi_add_packages_timed(&packages);
            black_box(duration_ns)
        })
    });
}

// Warm cache benchmarks - warm up cache before each run
fn bench_warm_cache_small(c: &mut Criterion) {
    let packages = ["numpy"];

    c.bench_function("warm_cache_small", |b| {
        b.iter(|| {
            warm_up_cache(&packages).expect("Failed to warm up cache");

            let duration_ns = pixi_add_packages_timed(&packages);
            black_box(duration_ns)
        })
    });
}

fn bench_warm_cache_medium(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "requests"];

    c.bench_function("warm_cache_medium", |b| {
        b.iter(|| {
            warm_up_cache(&packages).expect("Failed to warm up cache");

            let duration_ns = pixi_add_packages_timed(&packages);
            black_box(duration_ns)
        })
    });
}

fn bench_warm_cache_large(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "scipy", "matplotlib", "requests"];

    c.bench_function("warm_cache_large", |b| {
        b.iter(|| {
            warm_up_cache(&packages).expect("Failed to warm up cache");

            let duration_ns = pixi_add_packages_timed(&packages);
            black_box(duration_ns)
        })
    });
}

criterion_group!(
    benches,
    bench_cold_cache_small,
    bench_cold_cache_medium,
    bench_cold_cache_large,
    bench_warm_cache_small,
    bench_warm_cache_medium,
    bench_warm_cache_large,
);
criterion_main!(benches);
