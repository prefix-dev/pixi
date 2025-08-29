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

        let start = Instant::now();
        let output = cmd.output()?;
        let duration = start.elapsed();

        if !output.status.success() {
            eprintln!(
                "❌ pixi add failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            println!(
                "✅ Added {} packages in {:.2}s (isolated)",
                packages.len(),
                duration.as_secs_f64()
            );
        }

        Ok(duration)
    }

    /// Install packages to warm up the cache (without timing)
    fn warm_up_cache(&self, packages: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
        println!("🔥 Warming up isolated cache with packages: {:?}", packages);

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

        let output = cmd.output()?;

        if !output.status.success() {
            eprintln!(
                "Warning: cache warm-up failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            println!("✅ Isolated cache warmed up successfully");
        }

        Ok(())
    }

    /// Install packages again in the same environment (for warm cache benchmarking)
    fn pixi_add_packages_timed_warm(
        &self,
        packages: &[&str],
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        // Create a new project directory for the second installation
        let project_dir_2 = self._temp_dir.path().join("project2");
        fs::create_dir_all(&project_dir_2)?;

        // Create pixi.toml in the new project directory
        use std::fs::File;
        use std::io::Write;

        let pixi_toml = r#"[project]
name = "benchmark-project-2"
version = "0.1.0"
description = "Benchmark project for pixi add (warm cache test)"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

[dependencies]
"#;

        let mut file = File::create(project_dir_2.join("pixi.toml"))?;
        file.write_all(pixi_toml.as_bytes())?;

        let mut cmd = Command::new("pixi");
        cmd.arg("add").current_dir(&project_dir_2);

        for package in packages {
            cmd.arg(package);
        }

        // Set environment variables for isolation (same cache as warm-up)
        for (key, value) in self.get_env_vars() {
            cmd.env(key, value);
        }

        println!("⏱️ Timing: pixi add {} (warm cache)", packages.join(" "));

        let start = Instant::now();
        let output = cmd.output()?;
        let duration = start.elapsed();

        if !output.status.success() {
            eprintln!(
                "❌ pixi add failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            println!(
                "✅ Added {} packages in {:.2}s (warm cache)",
                packages.len(),
                duration.as_secs_f64()
            );
        }

        Ok(duration)
    }
}

// Cold cache benchmarks - each run gets a fresh isolated environment (already cold)
fn bench_small(c: &mut Criterion) {
    let packages = ["numpy"];
    let env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");

    c.bench_function("cold_cache_small", |b| {
        b.iter(|| {
            let duration = env
                .pixi_add_packages_timed(&packages)
                .expect("Failed to time pixi add");
            black_box(duration);
        })
    });

    // Warm up cache once before warm cache benchmarks
    env.warm_up_cache(&packages)
        .expect("Failed to warm up cache");

    c.bench_function("warm_cache_small", |b| {
        b.iter(|| {
            let duration = env
                .pixi_add_packages_timed_warm(&packages)
                .expect("Failed to time pixi add");
            black_box(duration);
        })
    });
}

fn bench_medium(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "requests", "click", "pyyaml"];
    let env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");

    c.bench_function("cold_cache_medium", |b| {
        b.iter(|| {
            let duration = env
                .pixi_add_packages_timed(&packages)
                .expect("Failed to time pixi add");
            black_box(duration);
        })
    });

    env.warm_up_cache(&packages)
        .expect("Failed to warm up cache");

    c.bench_function("warm_cache_medium", |b| {
        b.iter(|| {
            let duration = env
                .pixi_add_packages_timed_warm(&packages)
                .expect("Failed to time pixi add");
            black_box(duration);
        })
    });
}

fn bench_large(c: &mut Criterion) {
    let env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
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
            let duration = env
                .pixi_add_packages_timed(&packages)
                .expect("Failed to time pixi add");
            black_box(duration);
        })
    });

    env.warm_up_cache(&packages)
        .expect("Failed to warm up cache");

    c.bench_function("warm_cache_large", |b| {
        b.iter(|| {
            let duration = env
                .pixi_add_packages_timed_warm(&packages)
                .expect("Failed to time pixi add");
            black_box(duration);
        })
    });
}

criterion_group!(benches, bench_small, bench_medium, bench_large,);
criterion_main!(benches);
