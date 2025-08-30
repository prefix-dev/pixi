use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fs_err as fs;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Create an isolated pixi environment with temporary cache and home directories
struct IsolatedPixiEnv {
    _temp_dir: TempDir, // Keep temp dir alive
    cache_dir: PathBuf,
    home_dir: PathBuf,
    project_dir: PathBuf,
}

impl IsolatedPixiEnv {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let base_path = temp_dir.path();

        let cache_dir = base_path.join("pixi_cache");
        let home_dir = base_path.join("pixi_home");
        let project_dir = base_path.join("project");

        // Create the directories
        fs::create_dir_all(&cache_dir)?;
        fs::create_dir_all(&home_dir)?;
        fs::create_dir_all(&project_dir)?;

        Ok(Self {
            _temp_dir: temp_dir,
            cache_dir,
            home_dir,
            project_dir,
        })
    }

    /// Get environment variables for pixi isolation
    fn get_env_vars(&self) -> HashMap<String, String> {
        let mut env_vars = HashMap::new();

        // Set pixi cache directory to our temporary location
        env_vars.insert(
            "PIXI_CACHE_DIR".to_string(),
            self.cache_dir.to_string_lossy().to_string(),
        );

        // Set pixi home directory to our temporary location
        env_vars.insert(
            "PIXI_HOME".to_string(),
            self.home_dir.to_string_lossy().to_string(),
        );

        // Also set XDG cache dir as fallback
        env_vars.insert(
            "XDG_CACHE_HOME".to_string(),
            self.cache_dir.to_string_lossy().to_string(),
        );

        env_vars
    }

    /// Create a pixi project in the isolated environment
    fn create_pixi_project(&self) -> Result<(), Box<dyn std::error::Error>> {
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

        let mut file = File::create(self.project_dir.join("pixi.toml"))?;
        file.write_all(pixi_toml.as_bytes())?;
        Ok(())
    }

    /// Run pixi add command with timing in the isolated environment
    /// Times the entire process until all packages are actually installed
    fn pixi_add_packages_timed(
        &self,
        packages: &[&str],
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        self.create_pixi_project()?;

        let mut cmd = Command::new("pixi");
        cmd.arg("add").current_dir(&self.project_dir);

        for package in packages {
            cmd.arg(package);
        }

        // Set environment variables for isolation
        for (key, value) in self.get_env_vars() {
            cmd.env(key, value);
        }

        println!("⏱️ Timing: pixi add {} (isolated)", packages.join(" "));

        // Start timing from before the command execution
        let start = Instant::now();

        // Execute the pixi add command
        let output = cmd.output()?;

        if !output.status.success() {
            return Err(format!(
                "pixi add failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        // Wait until all packages are actually installed in the environment
        self.wait_for_packages_installed(packages)?;

        // Total time includes command execution + waiting for installation completion
        let duration = start.elapsed();

        println!(
            "✅ Added {} packages in {:.2}s (isolated)",
            packages.len(),
            duration.as_secs_f64()
        );

        Ok(duration)
    }

    /// Wait for all packages to be installed in the environment (polling every 100ms)
    fn wait_for_packages_installed(
        &self,
        packages: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::thread::sleep;
        use std::time::Duration as StdDuration;

        const MAX_RETRIES: u32 = 200; // 200 retries * 100ms = 20 seconds max wait time
        const RETRY_INTERVAL: StdDuration = StdDuration::from_millis(100);

        for retry in 0..MAX_RETRIES {
            // Run pixi list to check installed packages
            let mut cmd = Command::new("pixi");
            cmd.arg("list").current_dir(&self.project_dir);

            // Set environment variables for isolation
            for (key, value) in self.get_env_vars() {
                cmd.env(key, value);
            }

            let output = cmd.output();

            match output {
                Ok(output) if output.status.success() => {
                    let list_output = String::from_utf8_lossy(&output.stdout);

                    // Check if all packages are present in the output
                    let mut missing_packages = Vec::new();
                    for package in packages {
                        if !list_output.contains(package) {
                            missing_packages.push(*package);
                        }
                    }

                    if missing_packages.is_empty() {
                        println!("✅ All {} packages validated as installed via 'pixi list' after {} retries ({}ms)", 
                            packages.len(), retry + 1, (retry + 1) * 100);
                        return Ok(());
                    }

                    if retry == MAX_RETRIES - 1 {
                        return Err(format!(
                            "The following packages were not found in 'pixi list' output after {} retries ({}ms): {}\nOutput: {}",
                            MAX_RETRIES,
                            MAX_RETRIES * 100,
                            missing_packages.join(", "),
                            list_output
                        ).into());
                    }

                    // Only print status every 10 retries (every second) to avoid spam
                    if retry % 10 == 0 {
                        println!(
                            "⏳ Waiting for packages to be installed... ({}ms elapsed) - Missing: {}",
                            (retry + 1) * 100,
                            missing_packages.join(", ")
                        );
                    }
                }
                Ok(output) => {
                    // pixi list failed - environment might not be ready yet
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if retry == MAX_RETRIES - 1 {
                        return Err(format!(
                            "pixi list command failed after {} retries ({}ms): {}",
                            MAX_RETRIES,
                            MAX_RETRIES * 100,
                            stderr
                        )
                        .into());
                    }

                    // Only print error status every 10 retries to avoid spam
                    if retry % 10 == 0 {
                        println!(
                            "⏳ pixi list not ready yet... ({}ms elapsed)",
                            (retry + 1) * 100
                        );
                    }
                }
                Err(_) => {
                    // Command execution failed
                    if retry == MAX_RETRIES - 1 {
                        return Err(format!(
                            "Failed to execute pixi list command after {} retries ({}ms)",
                            MAX_RETRIES,
                            MAX_RETRIES * 100
                        )
                        .into());
                    }
                }
            }

            sleep(RETRY_INTERVAL);
        }

        Err("Package installation validation timed out".into())
    }
}

fn bench_small(c: &mut Criterion) {
    let packages = ["numpy"];

    c.bench_function("cold_cache_small", |b| {
        b.iter(|| {
            let env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .pixi_add_packages_timed(&packages)
                .expect("Failed to time pixi add");
            black_box(duration)
        })
    });

    let env2 = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
    env2.pixi_add_packages_timed(&packages)
        .expect("Failed to time pixi add");
    c.bench_function("warm_cache_small", |b| {
        b.iter(|| {
            let duration = env2
                .pixi_add_packages_timed(&packages)
                .expect("Failed to time pixi add");
            black_box(duration)
        })
    });
}

fn bench_medium(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "requests", "click", "pyyaml"];

    c.bench_function("cold_cache_medium", |b| {
        b.iter(|| {
            let env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .pixi_add_packages_timed(&packages)
                .expect("Failed to time pixi add");
            black_box(duration)
        })
    });

    let env2 = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
    env2.pixi_add_packages_timed(&packages)
        .expect("Failed to time pixi add");
    c.bench_function("warm_cache_medium", |b| {
        b.iter(|| {
            let duration = env2
                .pixi_add_packages_timed(&packages)
                .expect("Failed to time pixi add");
            black_box(duration)
        })
    });
}

fn bench_large(c: &mut Criterion) {
    let packages = [
        "pytorch",
        "scipy",
        "scikit-learn",
        "matplotlib",
        "jupyter",
        "bokeh",
        "dask",
        "xarray",
        "opencv",
        "pandas",
    ];

    c.bench_function("cold_cache_large", |b| {
        b.iter(|| {
            let env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .pixi_add_packages_timed(&packages)
                .expect("Failed to time pixi add");
            black_box(duration)
        })
    });

    let env2 = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
    env2.pixi_add_packages_timed(&packages)
        .expect("Failed to time pixi add");
    c.bench_function("warm_cache_large", |b| {
        b.iter(|| {
            let duration = env2
                .pixi_add_packages_timed(&packages)
                .expect("Failed to time pixi add");
            black_box(duration)
        })
    });
}

criterion_group!(benches, bench_small, bench_medium, bench_large);
criterion_main!(benches);
