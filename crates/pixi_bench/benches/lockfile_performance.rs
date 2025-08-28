use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn create_pixi_project_with_deps(
    project_dir: &Path,
    packages: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs::File;
    use std::io::Write;

    let mut dependencies = String::new();
    for package in packages {
        dependencies.push_str(&format!("{} = \"*\"\n", package));
    }

    let pixi_toml = format!(
        r#"[project]
name = "benchmark-project"
version = "0.1.0"
description = "Benchmark project for pixi"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

[dependencies]
{}"#,
        dependencies
    );

    let mut file = File::create(project_dir.join("pixi.toml"))?;
    file.write_all(pixi_toml.as_bytes())?;
    Ok(())
}

fn pixi_install_with_lockfile_generation(packages: &[&str]) -> Duration {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Create project with dependencies in pixi.toml
    create_pixi_project_with_deps(project_path, packages).expect("Failed to create pixi project");

    println!("⏱️ Timing pixi install (first time - generates lockfile)");

    // Time the full install process (includes lockfile generation)
    let start = Instant::now();

    let output = Command::new("pixi")
        .arg("install")
        .current_dir(project_path)
        .output()
        .expect("Failed to execute pixi install");

    let duration = start.elapsed();

    if !output.status.success() {
        eprintln!(
            "❌ pixi install failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    } else {
        println!(
            "✅ First install (with lockfile generation) took {:.2}s",
            duration.as_secs_f64()
        );
    }

    duration
}

fn pixi_install_with_existing_lockfile(packages: &[&str]) -> Duration {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Create project and generate lockfile first (not timed)
    create_pixi_project_with_deps(project_path, packages).expect("Failed to create pixi project");

    let _setup = Command::new("pixi")
        .arg("install")
        .current_dir(project_path)
        .output()
        .expect("Failed to generate lockfile");

    // Delete the environment but keep the lockfile
    let env_dir = project_path.join(".pixi");
    if env_dir.exists() {
        let _ = std::fs::remove_dir_all(&env_dir);
    }

    println!("⏱️ Timing pixi install (with existing lockfile)");

    // Now time the install with existing lockfile
    let start = Instant::now();

    let output = Command::new("pixi")
        .arg("install")
        .current_dir(project_path)
        .output()
        .expect("Failed to execute pixi install");

    let duration = start.elapsed();

    if !output.status.success() {
        eprintln!(
            "❌ pixi install with lockfile failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    } else {
        println!(
            "✅ Install with existing lockfile took {:.2}s",
            duration.as_secs_f64()
        );
    }

    duration
}

fn bench_lockfile_vs_no_lockfile_small(c: &mut Criterion) {
    let packages = ["numpy"];

    let mut group = c.benchmark_group("lockfile_small");

    group.bench_function("without_lockfile", |b| {
        b.iter(|| black_box(pixi_install_with_lockfile_generation(&packages)))
    });

    group.bench_function("with_lockfile", |b| {
        b.iter(|| black_box(pixi_install_with_existing_lockfile(&packages)))
    });

    group.finish();
}

fn bench_lockfile_vs_no_lockfile_medium(c: &mut Criterion) {
    let packages = ["pandas", "requests", "pyyaml"];

    let mut group = c.benchmark_group("lockfile_medium");

    group.bench_function("without_lockfile", |b| {
        b.iter(|| black_box(pixi_install_with_lockfile_generation(&packages)))
    });

    group.bench_function("with_lockfile", |b| {
        b.iter(|| black_box(pixi_install_with_existing_lockfile(&packages)))
    });

    group.finish();
}

fn bench_lockfile_vs_no_lockfile_large(c: &mut Criterion) {
    let packages = [
        "numpy",
        "pandas",
        "scipy",
        "matplotlib",
        "scikit-learn",
        "requests",
        "click",
        "flask",
        "jinja2",
        "pyyaml",
    ];

    let mut group = c.benchmark_group("lockfile_large");

    group.bench_function("without_lockfile", |b| {
        b.iter(|| black_box(pixi_install_with_lockfile_generation(&packages)))
    });

    group.bench_function("with_lockfile", |b| {
        b.iter(|| black_box(pixi_install_with_existing_lockfile(&packages)))
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_lockfile_vs_no_lockfile_small,
    bench_lockfile_vs_no_lockfile_medium,
    bench_lockfile_vs_no_lockfile_large
);
criterion_main!(benches);
