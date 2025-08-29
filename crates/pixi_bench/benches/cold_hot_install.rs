use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::runtime::Runtime;

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

async fn verify_installation_complete(
    project_path: &Path,
    packages: &[&str],
    timeout_secs: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::time::{interval, timeout, Duration as TokioDuration};

    println!("Verifying installation completion (polling every 100ms)...");

    let timeout_duration = TokioDuration::from_secs(timeout_secs);
    let mut interval = interval(TokioDuration::from_millis(100));

    let verification_result = timeout(timeout_duration, async {
        loop {
            interval.tick().await;

            // Check 1: Verify pixi.lock file exists
            let lock_file = project_path.join("pixi.lock");
            if !lock_file.exists() {
                println!("Waiting for pixi.lock file...");
                continue;
            }

            // Check 2: Verify packages are listed in pixi.toml
            let toml_content =
                match std::fs::File::open(project_path.join("pixi.toml")).and_then(|mut file| {
                    use std::io::Read;
                    let mut content = String::new();
                    file.read_to_string(&mut content)?;
                    Ok(content)
                }) {
                    Ok(content) => content,
                    Err(_) => {
                        println!("Waiting for pixi.toml to be readable...");
                        continue;
                    }
                };

            let mut all_packages_in_toml = true;
            for package in packages {
                if !toml_content.contains(package) {
                    all_packages_in_toml = false;
                    break;
                }
            }

            if !all_packages_in_toml {
                println!("Waiting for all packages to be added to pixi.toml...");
                continue;
            }

            // Check 3: Verify environment directory exists
            let env_dir = project_path.join(".pixi").join("envs");
            if !env_dir.exists() {
                println!("Waiting for environment directory to be created...");
                continue;
            }

            // Check 4: Try to run pixi list to verify packages are actually installed
            let list_output = match Command::new("pixi")
                .args(["list"])
                .current_dir(project_path)
                .output()
            {
                Ok(output) => output,
                Err(_) => {
                    println!("pixi list command failed, retrying...");
                    continue;
                }
            };

            if !list_output.status.success() {
                println!("pixi list returned error, packages may still be installing...");
                continue;
            }

            let list_content = String::from_utf8_lossy(&list_output.stdout);
            let mut all_packages_installed = true;

            for package in packages {
                if !list_content.contains(package) {
                    all_packages_installed = false;
                    break;
                }
            }

            if !all_packages_installed {
                println!("Not all packages found in pixi list output, waiting...");
                continue;
            }

            // All checks passed!
            println!("Installation verification completed successfully");
            return Ok(());
        }
    })
    .await;

    match verification_result {
        Ok(result) => result,
        Err(_) => Err(format!(
            "Installation verification timed out after {} seconds",
            timeout_secs
        )
        .into()),
    }
}

async fn pixi_add_packages_timed(
    packages: &[&str],
) -> Result<Duration, Box<dyn std::error::Error>> {
    let temp_dir = TempDir::new()?;
    let project_path = temp_dir.path();

    create_pixi_project(project_path)?;

    let mut cmd = Command::new("pixi");
    cmd.arg("add").current_dir(project_path);

    for package in packages {
        cmd.arg(package);
    }

    println!("Timing: pixi add {}", packages.join(" "));

    let start = Instant::now();

    // Start the pixi add command (non-blocking)
    let child = tokio::process::Command::new("pixi")
        .arg("add")
        .args(packages)
        .current_dir(project_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    // Start verification polling immediately (it will wait for files to appear)
    let project_path_clone = project_path.to_path_buf();
    let packages_clone: Vec<String> = packages.iter().map(|s| s.to_string()).collect();

    let verification_task = tokio::spawn(async move {
        let packages_refs: Vec<&str> = packages_clone.iter().map(|s| s.as_str()).collect();
        verify_installation_complete(&project_path_clone, &packages_refs, 300).await
    });

    // Wait for both the command to finish and verification to complete
    let command_future = async {
        let output = child.wait_with_output().await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "pixi add failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            ))
        }
    };

    command_future.await?;
    let verification_result = verification_task.await.map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Verification task failed: {}", e),
        )
    })?;

    // Handle the verification result which returns Box<dyn Error + Send + Sync>
    verification_result.map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("Verification failed: {}", e),
        )
    })?;

    let duration = start.elapsed();

    println!(
        "Added and verified {} packages in {:.2}s",
        packages.len(),
        duration.as_secs_f64()
    );

    Ok(duration)
}

// Cold cache benchmarks - clear cache before each run and time everything
fn bench_cold_cache_small(c: &mut Criterion) {
    let packages = ["numpy"];
    let rt = Runtime::new().unwrap();

    c.bench_function("cold_cache_small", |b| {
        b.iter(|| {
            // Clear cache and install - time the entire process
            clear_pixi_cache().expect("Failed to clear cache");
            std::thread::sleep(Duration::from_millis(500)); // Brief pause for cache clearing

            let start = Instant::now();
            match rt.block_on(pixi_add_packages_timed(&packages)) {
                Ok(_) => {
                    let total_duration = start.elapsed();
                    println!("Cold install took: {:.2}s", total_duration.as_secs_f64());
                    black_box(total_duration)
                }
                Err(e) => {
                    eprintln!("Install failed: {}", e);
                    black_box(start.elapsed())
                }
            }
        })
    });
}

fn bench_cold_cache_medium(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "requests"];
    let rt = Runtime::new().unwrap();

    c.bench_function("cold_cache_medium", |b| {
        b.iter(|| {
            clear_pixi_cache().expect("Failed to clear cache");
            std::thread::sleep(Duration::from_millis(500));

            let start = Instant::now();
            match rt.block_on(pixi_add_packages_timed(&packages)) {
                Ok(_) => {
                    let total_duration = start.elapsed();
                    println!("Cold install took: {:.2}s", total_duration.as_secs_f64());
                    black_box(total_duration)
                }
                Err(e) => {
                    eprintln!("Install failed: {}", e);
                    black_box(start.elapsed())
                }
            }
        })
    });
}

fn bench_cold_cache_large(c: &mut Criterion) {
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
    let rt = Runtime::new().unwrap();

    c.bench_function("cold_cache_large", |b| {
        b.iter(|| {
            clear_pixi_cache().expect("Failed to clear cache");
            std::thread::sleep(Duration::from_secs(1)); // Longer pause for larger package set

            let start = Instant::now();
            match rt.block_on(pixi_add_packages_timed(&packages)) {
                Ok(_) => {
                    let total_duration = start.elapsed();
                    println!("Cold install took: {:.2}s", total_duration.as_secs_f64());
                    black_box(total_duration)
                }
                Err(e) => {
                    eprintln!("Install failed: {}", e);
                    black_box(start.elapsed())
                }
            }
        })
    });
}

// Hot cache benchmarks - warm up cache once, then just time installs
fn bench_hot_cache_small(c: &mut Criterion) {
    let packages = ["numpy"];
    let rt = Runtime::new().unwrap();

    // Warm up the cache once before all iterations
    warm_up_cache(&packages).expect("Failed to warm up cache");

    c.bench_function("hot_cache_small", |b| {
        b.iter(|| {
            match rt.block_on(pixi_add_packages_timed(&packages)) {
                Ok(duration) => {
                    println!("Hot install took: {:.2}s", duration.as_secs_f64());
                    black_box(duration)
                }
                Err(e) => {
                    eprintln!("Install failed: {}", e);
                    // Add some penalty time for failed installs
                    black_box(Duration::from_secs(60))
                }
            }
        })
    });
}

fn bench_hot_cache_medium(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "requests"];
    let rt = Runtime::new().unwrap();

    warm_up_cache(&packages).expect("Failed to warm up cache");

    c.bench_function("hot_cache_medium", |b| {
        b.iter(|| match rt.block_on(pixi_add_packages_timed(&packages)) {
            Ok(duration) => {
                println!("Hot install took: {:.2}s", duration.as_secs_f64());
                black_box(duration)
            }
            Err(e) => {
                eprintln!("Install failed: {}", e);
                black_box(Duration::from_secs(60))
            }
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
    let rt = Runtime::new().unwrap();

    warm_up_cache(&packages).expect("Failed to warm up cache");

    c.bench_function("hot_cache_large", |b| {
        b.iter(|| match rt.block_on(pixi_add_packages_timed(&packages)) {
            Ok(duration) => {
                println!("Hot install took: {:.2}s", duration.as_secs_f64());
                black_box(duration)
            }
            Err(e) => {
                eprintln!("Install failed: {}", e);
                black_box(Duration::from_secs(60))
            }
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
