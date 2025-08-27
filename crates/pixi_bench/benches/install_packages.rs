use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::path::Path;
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

fn pixi_add_package(package: &str) -> Duration {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let project_path = temp_dir.path();

    // Create pixi project
    create_pixi_project(project_path).expect("Failed to create pixi project");

    // Time the pixi add command
    let start = Instant::now();

    let output = Command::new("pixi")
        .arg("add")
        .arg(package)
        .current_dir(project_path)
        .output()
        .expect("Failed to execute pixi add");

    let duration = start.elapsed();

    if !output.status.success() {
        eprintln!(
            "Warning: pixi add {} failed: {}",
            package,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    duration
}

fn bench_add_small(c: &mut Criterion) {
    // Check if pixi is installed
    if Command::new("pixi").arg("--version").output().is_err() {
        panic!("pixi is not installed or not in PATH. Install with: curl -fsSL https://pixi.sh/install.sh | bash");
    }

    c.bench_function("add_small", |b| {
        b.iter(|| {
            // Test multiple small packages and average the time
            let packages = ["click"];
            let mut total_duration = Duration::new(0, 0);

            for package in packages {
                let duration = pixi_add_package(black_box(package));
                total_duration += duration;
            }

            black_box(total_duration / packages.len() as u32)
        })
    });
}

fn bench_add_medium(c: &mut Criterion) {
    c.bench_function("add_medium", |b| {
        b.iter(|| {
            // Test multiple medium packages and average the time
            let packages = ["numpy", "pandas", "flask"];
            let mut total_duration = Duration::new(0, 0);

            for package in packages {
                let duration = pixi_add_package(black_box(package));
                total_duration += duration;
            }

            black_box(total_duration / packages.len() as u32)
        })
    });
}

fn bench_add_large(c: &mut Criterion) {
    c.bench_function("add_large", |b| {
        b.iter(|| {
            // Test multiple large packages and average the time
            let packages = [
                "matplotlib",
                "scipy",
                "jupyter",
                "pyyaml",
                "requests",
                "numpy",
                "pandas",
                "flask",
            ];
            let mut total_duration = Duration::new(0, 0);

            for package in packages {
                let duration = pixi_add_package(black_box(package));
                total_duration += duration;
            }

            black_box(total_duration / packages.len() as u32)
        })
    });
}

criterion_group!(benches, bench_add_small, bench_add_medium, bench_add_large);
criterion_main!(benches);
