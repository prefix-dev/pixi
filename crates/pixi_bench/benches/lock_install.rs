use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fs_err as fs;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

// Pixi crate imports for direct API usage
use pixi_cli::install;
use pixi_config::ConfigCli;

// Single global runtime for all benchmarks
static RUNTIME: Lazy<tokio::runtime::Runtime> =
    Lazy::new(|| tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime"));

/// Create an isolated pixi environment for lockfile testing
struct IsolatedPixiEnv {
    _temp_dir: TempDir,
    cache_dir: PathBuf,
    home_dir: PathBuf,
    project_dir: PathBuf,
    project_created: bool,
}

impl IsolatedPixiEnv {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let base_path = temp_dir.path();

        let cache_dir = base_path.join("pixi_cache");
        let home_dir = base_path.join("pixi_home");
        let project_dir = base_path.join("project");

        fs::create_dir_all(&cache_dir)?;
        fs::create_dir_all(&home_dir)?;
        fs::create_dir_all(&project_dir)?;

        Ok(Self {
            _temp_dir: temp_dir,
            cache_dir,
            home_dir,
            project_dir,
            project_created: false,
        })
    }

    fn get_env_vars(&self) -> HashMap<String, String> {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "PIXI_CACHE_DIR".to_string(),
            self.cache_dir.to_string_lossy().to_string(),
        );
        env_vars.insert(
            "PIXI_HOME".to_string(),
            self.home_dir.to_string_lossy().to_string(),
        );
        env_vars.insert(
            "XDG_CACHE_HOME".to_string(),
            self.cache_dir.to_string_lossy().to_string(),
        );
        env_vars
    }

    /// Ensure local channel exists, create it dynamically if missing (for CI robustness)
    fn ensure_local_channel_exists(
        &self,
        local_channel_dir: &Path,
        packages: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let noarch_dir = local_channel_dir.join("noarch");

        // If the channel already exists, we're good
        if noarch_dir.exists() && noarch_dir.join("repodata.json").exists() {
            return Ok(());
        }

        println!("🔧 Creating local conda channel for CI environment...");

        // Create the directory structure
        fs::create_dir_all(&noarch_dir)?;

        // Create repodata.json
        self.create_repodata_json(&noarch_dir, packages)?;

        // Create minimal conda packages
        self.create_conda_packages(&noarch_dir, packages)?;

        println!("✅ Local conda channel created successfully");
        Ok(())
    }

    /// Create repodata.json for the local channel
    fn create_repodata_json(
        &self,
        noarch_dir: &Path,
        packages: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::fs::File;
        use std::io::Write;

        let mut repodata = serde_json::json!({
            "info": {
                "subdir": "noarch"
            },
            "packages": {},
            "packages.conda": {},
            "removed": [],
            "repodata_version": 1
        });

        // Add each package to the repodata
        for package in packages {
            let package_filename = format!("{}-1.0.0-py_0.tar.bz2", package);
            repodata["packages"][&package_filename] = serde_json::json!({
                "build": "py_0",
                "build_number": 0,
                "depends": [],
                "license": "MIT",
                "name": package,
                "platform": null,
                "subdir": "noarch",
                "timestamp": 1640995200000i64,
                "version": "1.0.0"
            });
        }

        let mut file = File::create(noarch_dir.join("repodata.json"))?;
        file.write_all(serde_json::to_string_pretty(&repodata)?.as_bytes())?;
        Ok(())
    }

    /// Create minimal conda packages
    fn create_conda_packages(
        &self,
        noarch_dir: &Path,
        packages: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::fs::File;
        use std::io::Write;
        use std::process::Command as StdCommand;

        for package in packages {
            let package_filename = format!("{}-1.0.0-py_0.tar.bz2", package);
            let package_path = noarch_dir.join(&package_filename);

            // Create a temporary directory for package contents
            let temp_dir = tempfile::TempDir::new()?;
            let info_dir = temp_dir.path().join("info");
            fs::create_dir_all(&info_dir)?;

            // Create index.json
            let index_data = serde_json::json!({
                "name": package,
                "version": "1.0.0",
                "build": "py_0",
                "build_number": 0,
                "depends": [],
                "license": "MIT",
                "platform": null,
                "subdir": "noarch",
                "timestamp": 1640995200000i64
            });

            let mut index_file = File::create(info_dir.join("index.json"))?;
            index_file.write_all(serde_json::to_string_pretty(&index_data)?.as_bytes())?;

            // Create empty files list
            File::create(info_dir.join("files"))?.write_all(b"")?;

            // Create paths.json
            let paths_data = serde_json::json!({
                "paths": [],
                "paths_version": 1
            });
            let mut paths_file = File::create(info_dir.join("paths.json"))?;
            paths_file.write_all(serde_json::to_string_pretty(&paths_data)?.as_bytes())?;

            // Create the tar.bz2 package using system tar command
            let output = StdCommand::new("tar")
                .args([
                    "-cjf",
                    package_path.to_str().unwrap(),
                    "-C",
                    temp_dir.path().to_str().unwrap(),
                    "info",
                ])
                .output()?;

            if !output.status.success() {
                return Err(format!(
                    "Failed to create tar.bz2 package for {}: {}",
                    package,
                    String::from_utf8_lossy(&output.stderr)
                )
                .into());
            }
        }

        Ok(())
    }

    /// Create pixi project and generate lockfile
    async fn create_pixi_project_with_lockfile(
        &mut self,
        packages: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::fs::File;
        use std::io::Write;

        let current_dir = std::env::current_dir()?;
        let local_channel_dir = if current_dir.ends_with("pixi_bench") {
            current_dir.join("my-local-channel")
        } else {
            current_dir.join("crates/pixi_bench/my-local-channel")
        };

        // Ensure the local channel exists, create it if it doesn't
        self.ensure_local_channel_exists(&local_channel_dir, packages)?;

        let local_channel_url = format!("file://{}", local_channel_dir.to_string_lossy());

        let mut pixi_toml = format!(
            r#"[project]
name = "lockfile-benchmark-project"
version = "0.1.0"
description = "Benchmark project for pixi lockfile testing"
channels = ["{}"]
platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

[dependencies]
"#,
            local_channel_url
        );

        // Add all packages to dependencies
        for package in packages {
            pixi_toml.push_str(&format!("{} = \"==1.0.0\"\n", package));
        }

        let mut file = File::create(self.project_dir.join("pixi.toml"))?;
        file.write_all(pixi_toml.as_bytes())?;

        // Generate lockfile by running install once
        self.run_pixi_install_internal(packages).await?;

        self.project_created = true;
        Ok(())
    }

    /// Create pixi project without lockfile
    fn create_pixi_project_without_lockfile(
        &mut self,
        packages: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::fs::File;
        use std::io::Write;

        let current_dir = std::env::current_dir()?;
        let local_channel_dir = if current_dir.ends_with("pixi_bench") {
            current_dir.join("my-local-channel")
        } else {
            current_dir.join("crates/pixi_bench/my-local-channel")
        };

        // Ensure the local channel exists, create it if it doesn't
        self.ensure_local_channel_exists(&local_channel_dir, packages)?;

        let local_channel_url = format!("file://{}", local_channel_dir.to_string_lossy());

        let mut pixi_toml = format!(
            r#"[project]
name = "no-lockfile-benchmark-project"
version = "0.1.0"
description = "Benchmark project for pixi no-lockfile testing"
channels = ["{}"]
platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

[dependencies]
"#,
            local_channel_url
        );

        // Add all packages to dependencies
        for package in packages {
            pixi_toml.push_str(&format!("{} = \"==1.0.0\"\n", package));
        }

        let mut file = File::create(self.project_dir.join("pixi.toml"))?;
        file.write_all(pixi_toml.as_bytes())?;

        // Ensure no lockfile exists
        let lockfile_path = self.project_dir.join("pixi.lock");
        if lockfile_path.exists() {
            fs::remove_file(lockfile_path)?;
        }

        self.project_created = true;
        Ok(())
    }

    /// Install with existing lockfile - should be faster as dependency resolution is skipped
    async fn pixi_install_with_lockfile(
        &mut self,
        packages: &[&str],
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        // Create project with lockfile if not already created
        if !self.project_created {
            self.create_pixi_project_with_lockfile(packages).await?;
        }

        // Ensure lockfile exists
        let lockfile_path = self.project_dir.join("pixi.lock");
        if !lockfile_path.exists() {
            return Err("Lockfile does not exist for with-lockfile benchmark".into());
        }

        println!(
            "⏱️ Timing: pixi install with lockfile ({} packages)",
            packages.len()
        );
        self.run_pixi_install_timed(packages).await
    }

    /// Install without lockfile - requires full dependency resolution
    async fn pixi_install_without_lockfile(
        &mut self,
        packages: &[&str],
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        // Always create fresh project without lockfile
        self.project_created = false;
        self.create_pixi_project_without_lockfile(packages)?;

        // Ensure no lockfile exists
        let lockfile_path = self.project_dir.join("pixi.lock");
        if lockfile_path.exists() {
            fs::remove_file(lockfile_path)?;
        }

        println!(
            "⏱️ Timing: pixi install without lockfile ({} packages)",
            packages.len()
        );
        self.run_pixi_install_timed(packages).await
    }

    /// Internal install method for setup (not timed)
    async fn run_pixi_install_internal(
        &self,
        _packages: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Set environment variables for pixi
        for (key, value) in self.get_env_vars() {
            std::env::set_var(key, value);
        }

        // Change to project directory
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(&self.project_dir)?;

        // Create install arguments
        let install_args = install::Args {
            project_config: pixi_cli::cli_config::WorkspaceConfig::default(),
            lock_file_usage: pixi_cli::LockFileUsageConfig::default(),
            environment: None,
            config: ConfigCli::default(),
            all: false,
            skip: None,
            skip_with_deps: None,
            only: None,
        };

        // Execute pixi install directly
        let result = install::execute(install_args).await;

        // Restore original directory
        std::env::set_current_dir(original_dir)?;

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("pixi install failed: {}", e).into()),
        }
    }

    /// Run the actual pixi install command using direct API (timed)
    async fn run_pixi_install_timed(
        &self,
        _packages: &[&str],
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        // Set environment variables for pixi
        for (key, value) in self.get_env_vars() {
            std::env::set_var(key, value);
        }

        // Change to project directory
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(&self.project_dir)?;

        let start = Instant::now();

        // Create install arguments
        let install_args = install::Args {
            project_config: pixi_cli::cli_config::WorkspaceConfig::default(),
            lock_file_usage: pixi_cli::LockFileUsageConfig::default(),
            environment: None,
            config: ConfigCli::default(),
            all: false,
            skip: None,
            skip_with_deps: None,
            only: None,
        };

        // Execute pixi install directly
        let result = install::execute(install_args).await;

        // Restore original directory
        std::env::set_current_dir(original_dir)?;

        match result {
            Ok(_) => {
                let duration = start.elapsed();
                println!("✅ Completed in {:.2}s", duration.as_secs_f64());
                Ok(duration)
            }
            Err(e) => {
                println!("❌ pixi install failed: {}", e);
                Err(format!("pixi install failed: {}", e).into())
            }
        }
    }
}

fn bench_lockfile_small(c: &mut Criterion) {
    let packages = ["numpy"];

    let mut group = c.benchmark_group("small_lockfile_installs");
    group.measurement_time(Duration::from_secs(30)); // Allow 30 seconds for measurements
    group.sample_size(20); // Increase sample size to meet criterion requirements
    group.warm_up_time(Duration::from_secs(5)); // Warm up time

    // Install with lockfile - should be faster
    group.bench_function("with_lockfile_small", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .pixi_install_with_lockfile(&packages)
                .await
                .expect("Failed to time pixi install with lockfile");
            black_box(duration)
        })
    });

    // Install without lockfile - requires dependency resolution
    group.bench_function("without_lockfile_small", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .pixi_install_without_lockfile(&packages)
                .await
                .expect("Failed to time pixi install without lockfile");
            black_box(duration)
        })
    });
}

fn bench_lockfile_medium(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "requests", "click", "pyyaml"];

    let mut group = c.benchmark_group("medium_lockfile_installs");
    group.measurement_time(Duration::from_secs(60)); // 1 minute
    group.sample_size(15); // Increase sample size to meet criterion requirements
    group.warm_up_time(Duration::from_secs(10));

    group.bench_function("with_lockfile_medium", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .pixi_install_with_lockfile(&packages)
                .await
                .expect("Failed to time pixi install with lockfile");
            black_box(duration)
        })
    });

    group.bench_function("without_lockfile_medium", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .pixi_install_without_lockfile(&packages)
                .await
                .expect("Failed to time pixi install without lockfile");
            black_box(duration)
        })
    });
}

fn bench_lockfile_large(c: &mut Criterion) {
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

    let mut group = c.benchmark_group("large_lockfile_installs");
    group.measurement_time(Duration::from_secs(120)); // 2 minutes
    group.sample_size(10); // Minimum sample size to meet criterion requirements
    group.warm_up_time(Duration::from_secs(15));

    group.bench_function("with_lockfile_large", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .pixi_install_with_lockfile(&packages)
                .await
                .expect("Failed to time pixi install with lockfile");
            black_box(duration)
        })
    });

    group.bench_function("without_lockfile_large", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .pixi_install_without_lockfile(&packages)
                .await
                .expect("Failed to time pixi install without lockfile");
            black_box(duration)
        })
    });
}

criterion_group!(
    benches,
    bench_lockfile_small,
    bench_lockfile_medium,
    bench_lockfile_large
);
criterion_main!(benches);
