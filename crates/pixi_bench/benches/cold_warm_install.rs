use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fs_err as fs;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::process::Command;
use tokio::time::{sleep, Duration as TokioDuration};

/// Tokio async executor for criterion benchmarks
struct TokioExecutor;

impl criterion::async_executor::AsyncExecutor for TokioExecutor {
    fn block_on<T>(&self, future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Runtime::new().unwrap().block_on(future)
    }
}

/// Create an isolated pixi environment with shared cache for warm testing
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

    /// Create with shared cache directory for warm testing
    fn new_with_shared_cache(shared_cache_dir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let base_path = temp_dir.path();

        let home_dir = base_path.join("pixi_home");
        let project_dir = base_path.join("project");

        fs::create_dir_all(&home_dir)?;
        fs::create_dir_all(&project_dir)?;

        Ok(Self {
            _temp_dir: temp_dir,
            cache_dir: shared_cache_dir.to_path_buf(),
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

    /// Create pixi project only once
    fn ensure_pixi_project_created(
        &mut self,
        packages: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        if self.project_created {
            return Ok(());
        }

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
name = "benchmark-project"
version = "0.1.0"
description = "Benchmark project for pixi local channel benchmark"
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

        self.project_created = true;
        Ok(())
    }

    /// For cold cache: create new project and install
    async fn pixi_install_cold(
        &mut self,
        packages: &[&str],
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        // Always create fresh project for cold test
        self.project_created = false;
        self.ensure_pixi_project_created(packages)?;

        self.run_pixi_install(packages).await
    }

    /// For warm cache: reuse existing project and install
    async fn pixi_install_warm(
        &mut self,
        packages: &[&str],
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        // Ensure project exists (but don't recreate if already exists)
        self.ensure_pixi_project_created(packages)?;

        // For warm test, we measure re-installation or verification time
        // This simulates "pixi install" when packages are already resolved/cached
        self.run_pixi_install(packages).await
    }

    /// Run the actual pixi install command
    async fn run_pixi_install(
        &self,
        packages: &[&str],
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        let mut cmd = Command::new("pixi");
        cmd.arg("install").current_dir(&self.project_dir);

        for (key, value) in self.get_env_vars() {
            cmd.env(key, value);
        }

        println!("⏱️ Timing: pixi install {} packages", packages.len());

        let start = Instant::now();
        let output = cmd.output().await?;

        if !output.status.success() {
            return Err(format!(
                "pixi install failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        // Wait for packages to be verified as installed
        self.wait_for_packages_installed(packages).await?;

        let duration = start.elapsed();
        println!("✅ Completed in {:.2}s", duration.as_secs_f64());

        Ok(duration)
    }

    async fn wait_for_packages_installed(
        &self,
        packages: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        const MAX_RETRIES: u32 = 200;
        const RETRY_INTERVAL: TokioDuration = TokioDuration::from_millis(100);

        for retry in 0..MAX_RETRIES {
            let mut cmd = Command::new("pixi");
            cmd.arg("list").current_dir(&self.project_dir);

            for (key, value) in self.get_env_vars() {
                cmd.env(key, value);
            }

            let output = cmd.output().await;

            match output {
                Ok(output) if output.status.success() => {
                    let list_output = String::from_utf8_lossy(&output.stdout);

                    let mut missing_packages = Vec::new();
                    for package in packages {
                        if !list_output.contains(package) {
                            missing_packages.push(*package);
                        }
                    }

                    if missing_packages.is_empty() {
                        println!(
                            "✅ All {} packages validated after {} retries",
                            packages.len(),
                            retry + 1
                        );
                        return Ok(());
                    }

                    if retry % 10 == 0 {
                        println!("⏳ Missing packages: {}", missing_packages.join(", "));
                    }
                }
                _ => {
                    if retry % 10 == 0 {
                        println!("⏳ pixi list not ready yet...");
                    }
                }
            }

            sleep(RETRY_INTERVAL).await;
        }

        Err("Package installation validation timed out".into())
    }
}

/// Shared cache for warm testing
struct SharedCache {
    cache_dir: PathBuf,
    _temp_dir: TempDir,
}

impl SharedCache {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let cache_dir = temp_dir.path().join("shared_pixi_cache");
        fs::create_dir_all(&cache_dir)?;

        Ok(Self {
            cache_dir,
            _temp_dir: temp_dir,
        })
    }
}

fn bench_small(c: &mut Criterion) {
    let packages = ["numpy"];

    // Create shared cache for warm testing
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("small_package_installs");
    group.measurement_time(Duration::from_secs(60)); // Allow 1 minute for measurements
    group.sample_size(10); // Reduce sample size for long operations
    group.warm_up_time(Duration::from_secs(5)); // Warm up time

    // Cold cache benchmark - always creates new isolated environment
    group.bench_function("cold_cache_small", |b| {
        b.to_async(TokioExecutor).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .pixi_install_cold(&packages)
                .await
                .expect("Failed to time pixi install");
            black_box(duration)
        })
    });

    // Warm cache benchmark - reuses shared cache and may reuse project
    group.bench_function("warm_cache_small", |b| {
        b.to_async(TokioExecutor).iter(|| async {
            let mut env = IsolatedPixiEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .pixi_install_warm(&packages)
                .await
                .expect("Failed to time pixi install");
            black_box(duration)
        })
    });
}

fn bench_medium(c: &mut Criterion) {
    let packages = ["numpy", "pandas", "requests", "click", "pyyaml"];

    let shared_cache = SharedCache::new().expect("Failed to create shared cache");

    let mut group = c.benchmark_group("medium_package_installs");
    group.measurement_time(Duration::from_secs(90)); // 1.5 minutes
    group.sample_size(5); // Even fewer samples for medium complexity
    group.warm_up_time(Duration::from_secs(10));

    group.bench_function("cold_cache_medium", |b| {
        b.to_async(TokioExecutor).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .pixi_install_cold(&packages)
                .await
                .expect("Failed to time pixi install");
            black_box(duration)
        })
    });

    group.bench_function("warm_cache_medium", |b| {
        b.to_async(TokioExecutor).iter(|| async {
            let mut env = IsolatedPixiEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .pixi_install_warm(&packages)
                .await
                .expect("Failed to time pixi install");
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

    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("large_package_installs");
    group.measurement_time(Duration::from_secs(180)); // 3 minutes
    group.sample_size(3); // Very few samples for large operations
    group.warm_up_time(Duration::from_secs(15));

    group.bench_function("cold_cache_large", |b| {
        b.to_async(TokioExecutor).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .pixi_install_cold(&packages)
                .await
                .expect("Failed to time pixi install");
            black_box(duration)
        })
    });

    group.bench_function("warm_cache_large", |b| {
        b.to_async(TokioExecutor).iter(|| async {
            let mut env = IsolatedPixiEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .pixi_install_warm(&packages)
                .await
                .expect("Failed to time pixi install");
            black_box(duration)
        })
    });
}

criterion_group!(benches, bench_small, bench_medium, bench_large);
criterion_main!(benches);
