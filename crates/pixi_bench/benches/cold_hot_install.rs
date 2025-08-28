use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::process::Command;

async fn create_pixi_project(project_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::fs::File;
    use tokio::io::AsyncWriteExt;

    let pixi_toml = r#"[project]
name = "benchmark-project"
version = "0.1.0"
description = "Benchmark project for pixi add"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

[dependencies]
"#;

    let mut file = File::create(project_dir.join("pixi.toml")).await?;
    file.write_all(pixi_toml.as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

async fn clear_pixi_cache() -> Result<(), Box<dyn std::error::Error>> {
    println!("Clearing pixi cache...");

    let output = Command::new("pixi")
        .args(["clean", "cache", "-a"])
        .output()
        .await?;

    if !output.status.success() {
        eprintln!(
            "Warning: pixi clean cache failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Clear cache directories asynchronously
    let home_dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let cache_dirs = [
        format!("{}/.cache/pixi", home_dir),
        format!("{}/.pixi", home_dir),
    ];

    for cache_dir in &cache_dirs {
        let path = PathBuf::from(cache_dir);
        if path.exists() {
            println!("Removing cache directory: {:?}", path);
            #[allow(clippy::disallowed_methods)]
            let _ = tokio::fs::remove_dir_all(&path).await;
        }
    }

    println!("Cache cleared");
    Ok(())
}

async fn warm_cache_with_packages(packages: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    println!("Warming cache with packages: {:?}", packages);

    let temp_dir = TempDir::new()?;
    let project_path = temp_dir.path();
    create_pixi_project(project_path).await?;

    let mut cmd = Command::new("pixi");
    cmd.arg("add").current_dir(project_path);

    for package in packages {
        cmd.arg(package);
    }

    let output = cmd.output().await?;

    if !output.status.success() {
        eprintln!(
            "Cache warm-up failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Err("Failed to warm cache".into());
    }

    println!("Cache warmed successfully");
    Ok(())
}

async fn pixi_install_packages(packages: &[&str]) -> Result<Duration, Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let project_path = temp_dir.path();

    create_pixi_project(project_path).await?;

    let mut cmd = Command::new("pixi");
    cmd.arg("add").current_dir(project_path);

    for package in packages {
        cmd.arg(package);
    }

    println!("Starting installation: pixi add {}", packages.join(" "));

    let start = Instant::now();
    let output = cmd.output().await?;

    // Wait for command to complete and check success
    if !output.status.success() {
        let error_msg = String::from_utf8_lossy(&output.stderr);
        return Err(format!("pixi add failed: {}", error_msg).into());
    }

    // Only measure time after successful completion
    let duration = start.elapsed();

    // Verify installation actually worked by checking for created files
    let pixi_lock = project_path.join("pixi.lock");
    let pixi_env = project_path.join(".pixi");

    if !pixi_lock.exists() {
        return Err("Installation failed - no pixi.lock created".into());
    }

    if !pixi_env.exists() {
        return Err("Installation failed - no .pixi environment created".into());
    }

    println!(
        "Successfully installed {} packages in {:.2}s",
        packages.len(),
        duration.as_secs_f64()
    );
    Ok(duration)
}

// Cold cache benchmarks with async support
fn bench_cold_cache_small(c: &mut Criterion) {
    let packages = ["numpy"];
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("cold_cache_small", |b| {
        b.iter(|| {
            rt.block_on(async {
                // Clear cache and install - time the entire process
                if let Err(e) = clear_pixi_cache().await {
                    eprintln!("Failed to clear cache: {}", e);
                }

                tokio::time::sleep(Duration::from_millis(500)).await;

                match pixi_install_packages(&packages).await {
                    Ok(duration) => {
                        println!("Cold install took: {:.2}s", duration.as_secs_f64());
                        black_box(duration)
                    }
                    Err(e) => {
                        eprintln!("Install failed: {}", e);
                        black_box(Duration::from_secs(60)) // Penalty for failed install
                    }
                }
            })
        })
    });
}

fn bench_cold_cache_medium(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "requests"];
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("cold_cache_medium", |b| {
        b.iter(|| {
            rt.block_on(async {
                if let Err(e) = clear_pixi_cache().await {
                    eprintln!("Failed to clear cache: {}", e);
                }

                tokio::time::sleep(Duration::from_millis(500)).await;

                match pixi_install_packages(&packages).await {
                    Ok(duration) => {
                        println!("Cold install took: {:.2}s", duration.as_secs_f64());
                        black_box(duration)
                    }
                    Err(e) => {
                        eprintln!("Install failed: {}", e);
                        black_box(Duration::from_secs(60))
                    }
                }
            })
        })
    });
}

fn bench_cold_cache_large(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "scipy", "matplotlib", "requests"];
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("cold_cache_large", |b| {
        b.iter(|| {
            rt.block_on(async {
                if let Err(e) = clear_pixi_cache().await {
                    eprintln!("Failed to clear cache: {}", e);
                }

                tokio::time::sleep(Duration::from_secs(1)).await;

                match pixi_install_packages(&packages).await {
                    Ok(duration) => {
                        println!("Cold install took: {:.2}s", duration.as_secs_f64());
                        black_box(duration)
                    }
                    Err(e) => {
                        eprintln!("Install failed: {}", e);
                        black_box(Duration::from_secs(60))
                    }
                }
            })
        })
    });
}

// Hot cache benchmarks with async support
fn bench_hot_cache_small(c: &mut Criterion) {
    let packages = ["numpy"];
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Warm up the cache once before all iterations
    rt.block_on(async {
        if let Err(e) = warm_cache_with_packages(&packages).await {
            eprintln!("Failed to warm cache: {}", e);
        }
    });

    c.bench_function("hot_cache_small", |b| {
        b.iter(|| {
            rt.block_on(async {
                match pixi_install_packages(&packages).await {
                    Ok(duration) => {
                        println!("Hot install took: {:.2}s", duration.as_secs_f64());
                        black_box(duration)
                    }
                    Err(e) => {
                        eprintln!("Install failed: {}", e);
                        black_box(Duration::from_secs(60))
                    }
                }
            })
        })
    });
}

fn bench_hot_cache_medium(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "requests"];
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
        if let Err(e) = warm_cache_with_packages(&packages).await {
            eprintln!("Failed to warm cache: {}", e);
        }
    });

    c.bench_function("hot_cache_medium", |b| {
        b.iter(|| {
            rt.block_on(async {
                match pixi_install_packages(&packages).await {
                    Ok(duration) => {
                        println!("Hot install took: {:.2}s", duration.as_secs_f64());
                        black_box(duration)
                    }
                    Err(e) => {
                        eprintln!("Install failed: {}", e);
                        black_box(Duration::from_secs(60))
                    }
                }
            })
        })
    });
}

fn bench_hot_cache_large(c: &mut Criterion) {
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
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
        if let Err(e) = warm_cache_with_packages(&packages).await {
            eprintln!("Failed to warm cache: {}", e);
        }
    });

    c.bench_function("hot_cache_large", |b| {
        b.iter(|| {
            rt.block_on(async {
                match pixi_install_packages(&packages).await {
                    Ok(duration) => {
                        println!("Hot install took: {:.2}s", duration.as_secs_f64());
                        black_box(duration)
                    }
                    Err(e) => {
                        eprintln!("Install failed: {}", e);
                        black_box(Duration::from_secs(60))
                    }
                }
            })
        })
    });
}

criterion_group!(
    benches,
    bench_cold_cache_small,
    bench_cold_cache_medium,
    bench_cold_cache_large,
    bench_hot_cache_small,
    bench_hot_cache_medium,
    bench_hot_cache_large,
);
criterion_main!(benches);
