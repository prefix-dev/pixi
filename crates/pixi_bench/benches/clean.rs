use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fs_err as fs;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tempfile::TempDir;

// Pixi crate imports for direct API usage
use pixi_cli::{clean, install};
use pixi_config::ConfigCli;

// Single global runtime for all benchmarks
static RUNTIME: Lazy<tokio::runtime::Runtime> =
    Lazy::new(|| tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime"));

/// Create an isolated pixi workspace environment for clean testing
struct IsolatedPixiWorkspace {
    _temp_dir: TempDir,
    workspace_dir: PathBuf,
    cache_dir: PathBuf,
}

impl IsolatedPixiWorkspace {
    /// Create with shared cache directory for warm testing
    fn new_with_shared_cache(
        shared_cache_dir: &std::path::Path,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let workspace_dir = temp_dir.path().join("workspace");

        fs::create_dir_all(&workspace_dir)?;

        Ok(Self {
            _temp_dir: temp_dir,
            workspace_dir,
            cache_dir: shared_cache_dir.to_path_buf(),
        })
    }

    fn get_env_vars(&self) -> HashMap<String, String> {
        let mut env_vars = HashMap::new();
        env_vars.insert(
            "PIXI_CACHE_DIR".to_string(),
            self.cache_dir.to_string_lossy().to_string(),
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
        local_channel_dir: &std::path::Path,
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
        noarch_dir: &std::path::Path,
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
        noarch_dir: &std::path::Path,
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

    /// Create a basic pixi.toml file with specified dependencies using local channel
    fn create_pixi_toml(&self, dependencies: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
        let current_dir = std::env::current_dir()?;
        let local_channel_dir = if current_dir.ends_with("pixi_bench") {
            current_dir.join("my-local-channel")
        } else {
            current_dir.join("crates/pixi_bench/my-local-channel")
        };

        // Ensure the local channel exists, create it if it doesn't
        self.ensure_local_channel_exists(&local_channel_dir, dependencies)?;

        let local_channel_url = format!("file://{}", local_channel_dir.to_string_lossy());

        let pixi_toml_content = format!(
            r#"[project]
name = "test-project"
version = "0.1.0"
description = "Test project for pixi clean benchmarks"
channels = ["{}", "conda-forge"]

[dependencies]
{}

[tasks]
test = "echo 'test task'"
"#,
            local_channel_url,
            dependencies
                .iter()
                .map(|dep| format!("{} = \"*\"", dep))
                .collect::<Vec<_>>()
                .join("\n")
        );

        let pixi_toml_path = self.workspace_dir.join("pixi.toml");
        fs::write(pixi_toml_path, pixi_toml_content)?;
        Ok(())
    }

    /// Create a pixi.toml with multiple environments using local channel
    fn create_multi_env_pixi_toml(&self) -> Result<(), Box<dyn std::error::Error>> {
        let current_dir = std::env::current_dir()?;
        let local_channel_dir = if current_dir.ends_with("pixi_bench") {
            current_dir.join("my-local-channel")
        } else {
            current_dir.join("crates/pixi_bench/my-local-channel")
        };

        // All packages used in multi-environment setup
        let all_packages = [
            "python",
            "pytest",
            "pytest-cov",
            "black",
            "flake8",
            "mypy",
            "requests",
            "flask",
        ];

        // Ensure the local channel exists, create it if it doesn't
        self.ensure_local_channel_exists(&local_channel_dir, &all_packages)?;

        let local_channel_url = format!("file://{}", local_channel_dir.to_string_lossy());

        let pixi_toml_content = format!(
            r#"[project]
name = "multi-env-project"
version = "0.1.0"
description = "Multi-environment test project for pixi clean benchmarks"
channels = ["{}", "conda-forge"]

[dependencies]
python = "*"

[environments]
default = {{ solve-group = "default" }}
test = {{ features = ["test"], solve-group = "test" }}
dev = {{ features = ["dev"], solve-group = "dev" }}
prod = {{ features = ["prod"], solve-group = "prod" }}

[feature.test.dependencies]
pytest = "*"
pytest-cov = "*"

[feature.dev.dependencies]
black = "*"
flake8 = "*"
mypy = "*"

[feature.prod.dependencies]
requests = "*"
flask = "*"

[tasks]
test = "pytest"
lint = "flake8 ."
format = "black ."
"#,
            local_channel_url
        );

        let pixi_toml_path = self.workspace_dir.join("pixi.toml");
        fs::write(pixi_toml_path, pixi_toml_content)?;
        Ok(())
    }

    /// Install dependencies to create environments using pixi API directly
    async fn install_dependencies(&self) -> Result<(), Box<dyn std::error::Error>> {
        // Set environment variables for pixi
        for (key, value) in self.get_env_vars() {
            std::env::set_var(key, value);
        }

        // Change to workspace directory
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(&self.workspace_dir)?;

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

    /// Run pixi clean and measure execution time using pixi API directly
    async fn pixi_clean(
        &self,
        environment: Option<&str>,
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        let env_desc = environment.map_or("all environments".to_string(), |e| {
            format!("environment '{}'", e)
        });
        println!("⏱️ Timing: pixi clean {}", env_desc);

        // Set environment variables for pixi
        for (key, value) in self.get_env_vars() {
            std::env::set_var(key, value);
        }

        // Force non-interactive mode for benchmarks
        std::env::set_var("NO_COLOR", "1");
        std::env::set_var("PIXI_NO_PROGRESS", "1");
        std::env::set_var("CI", "1");

        // Change to workspace directory
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(&self.workspace_dir)?;

        let start = Instant::now();

        // Create clean arguments
        let mut clean_args: clean::Args = unsafe { std::mem::zeroed() };
        clean_args.workspace_config = pixi_cli::cli_config::WorkspaceConfig::default();
        clean_args.environment = environment.map(|s| s.to_string());
        clean_args.activation_cache = false;
        clean_args.build = false;

        // Execute pixi clean directly
        let result = clean::execute(clean_args).await;

        // Restore original directory
        std::env::set_current_dir(original_dir)?;

        let duration = start.elapsed();

        match result {
            Ok(_) => {
                println!("✅ Clean completed in {:.2}s", duration.as_secs_f64());
                Ok(duration)
            }
            Err(e) => {
                println!("❌ pixi clean failed: {}", e);
                Err(format!("pixi clean failed: {}", e).into())
            }
        }
    }

    /// Check if environments exist
    fn environments_exist(&self) -> bool {
        self.workspace_dir.join(".pixi").join("envs").exists()
    }

    /// Get size of .pixi/envs directory
    fn get_envs_size(&self) -> Result<u64, Box<dyn std::error::Error>> {
        let envs_dir = self.workspace_dir.join(".pixi").join("envs");
        if !envs_dir.exists() {
            return Ok(0);
        }

        let mut total_size = 0;
        for entry in fs::read_dir(&envs_dir)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                total_size += metadata.len();
            } else if metadata.is_dir() {
                total_size += self.get_dir_size(&entry.path())?;
            }
        }
        Ok(total_size)
    }

    #[allow(clippy::only_used_in_recursion)]
    fn get_dir_size(&self, dir: &std::path::Path) -> Result<u64, Box<dyn std::error::Error>> {
        let mut total_size = 0;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                total_size += metadata.len();
            } else if metadata.is_dir() {
                total_size += self.get_dir_size(&entry.path())?;
            }
        }
        Ok(total_size)
    }

    /// Clean small environment (few small packages)
    async fn clean_small_environment(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.create_pixi_toml(&["python"])?;
        self.install_dependencies().await?;
        self.pixi_clean(None).await
    }

    /// Clean medium environment (several packages)
    async fn clean_medium_environment(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.create_pixi_toml(&["python", "numpy", "pandas", "requests"])?;
        self.install_dependencies().await?;
        self.pixi_clean(None).await
    }

    /// Clean large environment (many packages)
    async fn clean_large_environment(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.create_pixi_toml(&[
            "python",
            "numpy",
            "pandas",
            "scipy",
            "matplotlib",
            "jupyter",
            "scikit-learn",
            "requests",
            "flask",
            "django",
        ])?;
        self.install_dependencies().await?;
        self.pixi_clean(None).await
    }

    /// Clean specific environment from multi-environment setup
    async fn clean_specific_environment(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.create_multi_env_pixi_toml()?;
        // Install all environments first (pixi install installs all environments by default)
        self.install_dependencies().await?;

        // Clean only the test environment
        self.pixi_clean(Some("test")).await
    }

    /// Clean all environments from multi-environment setup
    async fn clean_multi_environments(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.create_multi_env_pixi_toml()?;
        // Install all environments first (pixi install installs all environments by default)
        self.install_dependencies().await?;

        // Clean all environments
        self.pixi_clean(None).await
    }

    /// Clean empty workspace (no environments to clean)
    async fn clean_empty_workspace(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.create_pixi_toml(&["python"])?;
        // Don't install dependencies, so no environments exist
        self.pixi_clean(None).await
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

fn bench_environment_sizes(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("environment_sizes_clean");
    group.measurement_time(Duration::from_secs(90)); // 1.5 minutes
    group.sample_size(8); // Moderate sample size
    group.warm_up_time(Duration::from_secs(10));

    // Small environment clean
    group.bench_function("clean_small_environment", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");
            let duration = RUNTIME
                .block_on(workspace.clean_small_environment())
                .expect("Failed to time pixi clean");
            black_box(duration)
        })
    });

    // Medium environment clean
    group.bench_function("clean_medium_environment", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");
            let duration = RUNTIME
                .block_on(workspace.clean_medium_environment())
                .expect("Failed to time pixi clean");
            black_box(duration)
        })
    });

    // Large environment clean
    group.bench_function("clean_large_environment", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");
            let duration = RUNTIME
                .block_on(workspace.clean_large_environment())
                .expect("Failed to time pixi clean");
            black_box(duration)
        })
    });
}

fn bench_multi_environment_scenarios(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("multi_environment_clean");
    group.measurement_time(Duration::from_secs(120)); // 2 minutes
    group.sample_size(10); // Minimum required sample size
    group.warm_up_time(Duration::from_secs(15));

    // Clean specific environment
    group.bench_function("clean_specific_environment", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");
            let duration = RUNTIME
                .block_on(workspace.clean_specific_environment())
                .expect("Failed to time pixi clean specific environment");
            black_box(duration)
        })
    });

    // Clean all environments in multi-environment setup
    group.bench_function("clean_all_multi_environments", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");
            let duration = RUNTIME
                .block_on(workspace.clean_multi_environments())
                .expect("Failed to time pixi clean all environments");
            black_box(duration)
        })
    });
}

fn bench_edge_cases(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("edge_cases_clean");
    group.measurement_time(Duration::from_secs(60)); // 1 minute
    group.sample_size(10); // More samples for quick operations
    group.warm_up_time(Duration::from_secs(5));

    // Clean empty workspace (no environments exist)
    group.bench_function("clean_empty_workspace", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");
            let duration = RUNTIME
                .block_on(workspace.clean_empty_workspace())
                .expect("Failed to time pixi clean empty workspace");
            black_box(duration)
        })
    });
}

fn bench_repeated_clean_operations(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("repeated_clean_operations");
    group.measurement_time(Duration::from_secs(90)); // 1.5 minutes
    group.sample_size(8); // Moderate sample size
    group.warm_up_time(Duration::from_secs(10));

    // Clean, reinstall, clean again cycle
    group.bench_function("clean_reinstall_clean_cycle", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");

            RUNTIME.block_on(async {
                // Setup environment
                workspace
                    .create_pixi_toml(&["python", "numpy"])
                    .expect("Failed to create pixi.toml");
                workspace
                    .install_dependencies()
                    .await
                    .expect("Failed to install dependencies");

                // First clean
                let duration1 = workspace
                    .pixi_clean(None)
                    .await
                    .expect("Failed to clean first time");

                // Reinstall
                workspace
                    .install_dependencies()
                    .await
                    .expect("Failed to reinstall dependencies");

                // Second clean
                let duration2 = workspace
                    .pixi_clean(None)
                    .await
                    .expect("Failed to clean second time");

                black_box((duration1, duration2))
            })
        })
    });
}

fn bench_clean_performance_by_size(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("clean_performance_by_size");
    group.measurement_time(Duration::from_secs(120)); // 2 minutes
    group.sample_size(10); // Minimum required sample size
    group.warm_up_time(Duration::from_secs(15));

    // Measure clean performance vs environment size
    group.bench_function("clean_with_size_measurement", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");

            RUNTIME.block_on(async {
                // Create large environment
                workspace
                    .create_pixi_toml(&[
                        "python",
                        "numpy",
                        "pandas",
                        "scipy",
                        "matplotlib",
                        "jupyter",
                        "scikit-learn",
                        "requests",
                        "flask",
                    ])
                    .expect("Failed to create pixi.toml");
                workspace
                    .install_dependencies()
                    .await
                    .expect("Failed to install dependencies");

                // Measure size before clean
                let size_before = workspace
                    .get_envs_size()
                    .expect("Failed to get environment size");

                // Clean and measure time
                let clean_duration = workspace
                    .pixi_clean(None)
                    .await
                    .expect("Failed to clean environment");

                // Verify environments are gone
                let environments_exist_after = workspace.environments_exist();

                black_box((clean_duration, size_before, environments_exist_after))
            })
        })
    });
}

criterion_group!(
    benches,
    bench_environment_sizes,
    bench_multi_environment_scenarios,
    bench_edge_cases,
    bench_repeated_clean_operations,
    bench_clean_performance_by_size
);
criterion_main!(benches);
