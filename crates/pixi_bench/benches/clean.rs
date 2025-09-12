use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fs_err as fs;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

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

    /// Create a basic pixi.toml file with specified dependencies
    fn create_pixi_toml(&self, dependencies: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
        let pixi_toml_content = format!(
            r#"[project]
name = "test-project"
version = "0.1.0"
description = "Test project for pixi clean benchmarks"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

[dependencies]
{}

[tasks]
test = "echo 'test task'"
"#,
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

    /// Create a pixi.toml with multiple environments
    fn create_multi_env_pixi_toml(&self) -> Result<(), Box<dyn std::error::Error>> {
        let pixi_toml_content = r#"[project]
name = "multi-env-project"
version = "0.1.0"
description = "Multi-environment test project for pixi clean benchmarks"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

[dependencies]
python = ">=3.8"

[environments]
default = { solve-group = "default" }
test = { features = ["test"], solve-group = "test" }
dev = { features = ["dev"], solve-group = "dev" }
prod = { features = ["prod"], solve-group = "prod" }

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
"#;

        let pixi_toml_path = self.workspace_dir.join("pixi.toml");
        fs::write(pixi_toml_path, pixi_toml_content)?;
        Ok(())
    }

    /// Install dependencies to create environments
    fn install_dependencies(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut cmd = Command::new("pixi");
        cmd.arg("install").current_dir(&self.workspace_dir);

        // Set environment variables
        for (key, value) in self.get_env_vars() {
            cmd.env(key, value);
        }

        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("pixi install failed: {}", stderr).into());
        }

        Ok(())
    }

    /// Install specific environment
    fn install_environment(&self, env_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut cmd = Command::new("pixi");
        cmd.arg("install")
            .arg("--environment")
            .arg(env_name)
            .current_dir(&self.workspace_dir);

        // Set environment variables
        for (key, value) in self.get_env_vars() {
            cmd.env(key, value);
        }

        let output = cmd.output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("pixi install {} failed: {}", env_name, stderr).into());
        }

        Ok(())
    }

    /// Run pixi clean and measure execution time
    fn pixi_clean(
        &self,
        environment: Option<&str>,
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        let env_desc = environment.map_or("all environments".to_string(), |e| {
            format!("environment '{}'", e)
        });
        println!("⏱️ Timing: pixi clean {}", env_desc);

        let start = Instant::now();

        let mut cmd = Command::new("pixi");
        cmd.arg("clean").current_dir(&self.workspace_dir);

        // Add environment-specific flag if specified
        if let Some(env_name) = environment {
            cmd.arg("--environment").arg(env_name);
        }

        // Set environment variables
        for (key, value) in self.get_env_vars() {
            cmd.env(key, value);
        }

        let result = cmd.output();

        match result {
            Ok(output) => {
                let duration = start.elapsed();
                if output.status.success() {
                    println!("✅ Clean completed in {:.2}s", duration.as_secs_f64());
                    Ok(duration)
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    println!("❌ pixi clean failed: {}", stderr);
                    Err(format!("pixi clean failed: {}", stderr).into())
                }
            }
            Err(e) => {
                println!("❌ pixi clean failed to execute: {}", e);
                Err(format!("pixi clean failed to execute: {}", e).into())
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
    fn clean_small_environment(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.create_pixi_toml(&["python"])?;
        self.install_dependencies()?;
        self.pixi_clean(None)
    }

    /// Clean medium environment (several packages)
    fn clean_medium_environment(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.create_pixi_toml(&["python", "numpy", "pandas", "requests"])?;
        self.install_dependencies()?;
        self.pixi_clean(None)
    }

    /// Clean large environment (many packages)
    fn clean_large_environment(&self) -> Result<Duration, Box<dyn std::error::Error>> {
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
        self.install_dependencies()?;
        self.pixi_clean(None)
    }

    /// Clean specific environment from multi-environment setup
    fn clean_specific_environment(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.create_multi_env_pixi_toml()?;
        // Install all environments first
        self.install_dependencies()?;
        self.install_environment("test")?;
        self.install_environment("dev")?;
        self.install_environment("prod")?;

        // Clean only the test environment
        self.pixi_clean(Some("test"))
    }

    /// Clean all environments from multi-environment setup
    fn clean_multi_environments(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.create_multi_env_pixi_toml()?;
        // Install all environments first
        self.install_dependencies()?;
        self.install_environment("test")?;
        self.install_environment("dev")?;
        self.install_environment("prod")?;

        // Clean all environments
        self.pixi_clean(None)
    }

    /// Clean empty workspace (no environments to clean)
    fn clean_empty_workspace(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.create_pixi_toml(&["python"])?;
        // Don't install dependencies, so no environments exist
        self.pixi_clean(None)
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
            let duration = workspace
                .clean_small_environment()
                .expect("Failed to time pixi clean");
            black_box(duration)
        })
    });

    // Medium environment clean
    group.bench_function("clean_medium_environment", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");
            let duration = workspace
                .clean_medium_environment()
                .expect("Failed to time pixi clean");
            black_box(duration)
        })
    });

    // Large environment clean
    group.bench_function("clean_large_environment", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");
            let duration = workspace
                .clean_large_environment()
                .expect("Failed to time pixi clean");
            black_box(duration)
        })
    });
}

fn bench_multi_environment_scenarios(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("multi_environment_clean");
    group.measurement_time(Duration::from_secs(120)); // 2 minutes
    group.sample_size(6); // Fewer samples for complex scenarios
    group.warm_up_time(Duration::from_secs(15));

    // Clean specific environment
    group.bench_function("clean_specific_environment", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");
            let duration = workspace
                .clean_specific_environment()
                .expect("Failed to time pixi clean specific environment");
            black_box(duration)
        })
    });

    // Clean all environments in multi-environment setup
    group.bench_function("clean_all_multi_environments", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");
            let duration = workspace
                .clean_multi_environments()
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
            let duration = workspace
                .clean_empty_workspace()
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

            // Setup environment
            workspace
                .create_pixi_toml(&["python", "numpy"])
                .expect("Failed to create pixi.toml");
            workspace
                .install_dependencies()
                .expect("Failed to install dependencies");

            // First clean
            let duration1 = workspace
                .pixi_clean(None)
                .expect("Failed to clean first time");

            // Reinstall
            workspace
                .install_dependencies()
                .expect("Failed to reinstall dependencies");

            // Second clean
            let duration2 = workspace
                .pixi_clean(None)
                .expect("Failed to clean second time");

            black_box((duration1, duration2))
        })
    });
}

fn bench_clean_performance_by_size(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("clean_performance_by_size");
    group.measurement_time(Duration::from_secs(120)); // 2 minutes
    group.sample_size(5); // Fewer samples for detailed analysis
    group.warm_up_time(Duration::from_secs(15));

    // Measure clean performance vs environment size
    group.bench_function("clean_with_size_measurement", |b| {
        b.iter(|| {
            let workspace = IsolatedPixiWorkspace::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create workspace with shared cache");

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
                .expect("Failed to install dependencies");

            // Measure size before clean
            let size_before = workspace
                .get_envs_size()
                .expect("Failed to get environment size");

            // Clean and measure time
            let clean_duration = workspace
                .pixi_clean(None)
                .expect("Failed to clean environment");

            // Verify environments are gone
            let environments_exist_after = workspace.environments_exist();

            black_box((clean_duration, size_before, environments_exist_after))
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
