use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fs_err as fs;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, Instant};
use tempfile::TempDir;

// Pixi crate imports for direct API usage
use pixi_global::project::GlobalSpec;
use pixi_global::{EnvironmentName, Project};
use rattler_conda_types::{NamedChannelOrUrl, Platform};

// Single global runtime for all benchmarks
static RUNTIME: Lazy<tokio::runtime::Runtime> =
    Lazy::new(|| tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime"));

/// Create an isolated pixi environment for global install testing
struct IsolatedPixiGlobalEnv {
    _temp_dir: TempDir,
    cache_dir: PathBuf,
    home_dir: PathBuf,
    global_dir: PathBuf,
}

impl IsolatedPixiGlobalEnv {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let base_path = temp_dir.path();

        let cache_dir = base_path.join("pixi_cache");
        let home_dir = base_path.join("pixi_home");
        let global_dir = base_path.join("pixi_global");

        fs::create_dir_all(&cache_dir)?;
        fs::create_dir_all(&home_dir)?;
        fs::create_dir_all(&global_dir)?;

        Ok(Self {
            _temp_dir: temp_dir,
            cache_dir,
            home_dir,
            global_dir,
        })
    }

    /// Create with shared cache directory for warm testing
    fn new_with_shared_cache(
        shared_cache_dir: &std::path::Path,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let base_path = temp_dir.path();

        let home_dir = base_path.join("pixi_home");
        let global_dir = base_path.join("pixi_global");

        fs::create_dir_all(&home_dir)?;
        fs::create_dir_all(&global_dir)?;

        Ok(Self {
            _temp_dir: temp_dir,
            cache_dir: shared_cache_dir.to_path_buf(),
            home_dir,
            global_dir,
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
            "PIXI_GLOBAL_DIR".to_string(),
            self.global_dir.to_string_lossy().to_string(),
        );
        env_vars.insert(
            "XDG_CACHE_HOME".to_string(),
            self.cache_dir.to_string_lossy().to_string(),
        );
        env_vars
    }

    /// Run pixi global install and measure execution time using pixi_global crate directly
    async fn pixi_global_install(
        &self,
        packages: &[&str],
        channels: Option<Vec<NamedChannelOrUrl>>,
        platform: Option<Platform>,
        _force_reinstall: bool,
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        println!("⏱️ Timing: pixi global install {} packages", packages.len());

        let start = Instant::now();

        // Set environment variables for pixi_global
        for (key, value) in self.get_env_vars() {
            std::env::set_var(key, value);
        }

        // Create or discover the global project
        let mut project = Project::discover_or_create().await?;

        // Create environment name from first package
        let env_name = EnvironmentName::from_str(&format!("bench_{}", packages[0]))?;

        // Use local channel if no channels specified
        let channels = channels.unwrap_or_else(|| {
            let current_dir = std::env::current_dir().unwrap_or_default();
            let local_channel_dir = if current_dir.ends_with("pixi_bench") {
                current_dir.join("my-local-channel")
            } else {
                current_dir.join("crates/pixi_bench/my-local-channel")
            };
            let local_channel_url = format!("file://{}", local_channel_dir.to_string_lossy());
            vec![NamedChannelOrUrl::Url(local_channel_url.parse().unwrap())]
        });

        // Add environment to manifest with channels
        project
            .manifest
            .add_environment(&env_name, Some(channels))?;

        // Set platform if specified
        if let Some(platform) = platform {
            project.manifest.set_platform(&env_name, platform)?;
        }

        // Add each package as a dependency with version constraint to match local channel
        for package in packages {
            let package_spec = format!("{}==1.0.0", package);
            let global_spec =
                GlobalSpec::try_from_str(&package_spec, project.global_channel_config())?;
            project.manifest.add_dependency(&env_name, &global_spec)?;
        }

        // Install the environment
        let _environment_update = project.install_environment(&env_name).await?;

        let duration = start.elapsed();
        println!(
            "✅ Global install completed in {:.2}s",
            duration.as_secs_f64()
        );

        Ok(duration)
    }

    /// Install a single small package
    async fn install_single_small(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.pixi_global_install(&["numpy"], None, None, false)
            .await
    }

    /// Install multiple small packages
    async fn install_multiple_small(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.pixi_global_install(&["numpy", "pandas", "requests"], None, None, false)
            .await
    }

    /// Install a medium-sized package
    async fn install_medium(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.pixi_global_install(&["matplotlib"], None, None, false)
            .await
    }

    /// Install a large package
    async fn install_large(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.pixi_global_install(&["jupyter"], None, None, false)
            .await
    }

    /// Install with force reinstall
    async fn install_with_force_reinstall(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        // First install
        let _ = self
            .pixi_global_install(&["numpy"], None, None, false)
            .await?;
        // Then force reinstall
        self.pixi_global_install(&["numpy"], None, None, true).await
    }

    /// Install with specific platform
    async fn install_with_platform(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        let platform = Platform::current();
        self.pixi_global_install(&["click"], None, Some(platform), false)
            .await
    }

    /// Install with custom channel
    async fn install_with_custom_channel(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        // Use local channel for this test too, but with different packages
        let current_dir = std::env::current_dir().unwrap_or_default();
        let local_channel_dir = if current_dir.ends_with("pixi_bench") {
            current_dir.join("my-local-channel")
        } else {
            current_dir.join("crates/pixi_bench/my-local-channel")
        };
        let local_channel_url = format!("file://{}", local_channel_dir.to_string_lossy());
        let channels = vec![NamedChannelOrUrl::Url(local_channel_url.parse().unwrap())];
        self.pixi_global_install(&["scipy"], Some(channels), None, false)
            .await
    }

    /// Install and uninstall a single small package (only uninstall is timed)
    async fn install_and_uninstall_single_small(
        &self,
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        // Set environment variables once for both operations
        for (key, value) in self.get_env_vars() {
            std::env::set_var(key, value);
        }

        // Create a single project instance for both operations
        let mut project = Project::discover_or_create().await?;
        let env_name = EnvironmentName::from_str("bench_numpy")?;

        // Use local channel
        let current_dir = std::env::current_dir().unwrap_or_default();
        let local_channel_dir = if current_dir.ends_with("pixi_bench") {
            current_dir.join("my-local-channel")
        } else {
            current_dir.join("crates/pixi_bench/my-local-channel")
        };
        let local_channel_url = format!("file://{}", local_channel_dir.to_string_lossy());
        let channels = vec![NamedChannelOrUrl::Url(local_channel_url.parse().unwrap())];

        // Setup: Install the package (not timed)
        project
            .manifest
            .add_environment(&env_name, Some(channels))?;
        let package_spec = "numpy==1.0.0";
        let global_spec = GlobalSpec::try_from_str(package_spec, project.global_channel_config())?;
        project.manifest.add_dependency(&env_name, &global_spec)?;
        let _ = project.install_environment(&env_name).await?;

        // Measure: Only the uninstall operation
        println!("⏱️ Timing: pixi global uninstall 1 packages");
        let start = Instant::now();
        let _ = project.remove_environment(&env_name).await?;
        let duration = start.elapsed();
        println!(
            "✅ Global uninstall completed in {:.2}s",
            duration.as_secs_f64()
        );

        Ok(duration)
    }

    /// Install and uninstall multiple small packages (only uninstall is timed)
    async fn install_and_uninstall_multiple_small(
        &self,
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        // Set environment variables once for all operations
        for (key, value) in self.get_env_vars() {
            std::env::set_var(key, value);
        }

        // Create a single project instance for all operations
        let mut project = Project::discover_or_create().await?;

        // Use local channel
        let current_dir = std::env::current_dir().unwrap_or_default();
        let local_channel_dir = if current_dir.ends_with("pixi_bench") {
            current_dir.join("my-local-channel")
        } else {
            current_dir.join("crates/pixi_bench/my-local-channel")
        };
        let local_channel_url = format!("file://{}", local_channel_dir.to_string_lossy());
        let channels = vec![NamedChannelOrUrl::Url(local_channel_url.parse().unwrap())];

        // Setup: Install the packages (not timed)
        let packages = ["numpy", "pandas", "requests"];
        for package in &packages {
            let env_name = EnvironmentName::from_str(&format!("bench_{}", package))?;
            project
                .manifest
                .add_environment(&env_name, Some(channels.clone()))?;
            let package_spec = format!("{}==1.0.0", package);
            let global_spec =
                GlobalSpec::try_from_str(&package_spec, project.global_channel_config())?;
            project.manifest.add_dependency(&env_name, &global_spec)?;
            let _ = project.install_environment(&env_name).await?;
        }

        // Measure: Only the uninstall operations
        println!(
            "⏱️ Timing: pixi global uninstall {} packages",
            packages.len()
        );
        let start = Instant::now();
        for package in &packages {
            let env_name = EnvironmentName::from_str(&format!("bench_{}", package))?;
            let _ = project.remove_environment(&env_name).await?;
        }
        let duration = start.elapsed();
        println!(
            "✅ Multiple uninstall completed in {:.2}s",
            duration.as_secs_f64()
        );
        Ok(duration)
    }

    /// Install and uninstall a medium-sized package (only uninstall is timed)
    async fn install_and_uninstall_medium(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        // Set environment variables once for both operations
        for (key, value) in self.get_env_vars() {
            std::env::set_var(key, value);
        }

        // Create a single project instance for both operations
        let mut project = Project::discover_or_create().await?;
        let env_name = EnvironmentName::from_str("bench_matplotlib")?;

        // Use local channel
        let current_dir = std::env::current_dir().unwrap_or_default();
        let local_channel_dir = if current_dir.ends_with("pixi_bench") {
            current_dir.join("my-local-channel")
        } else {
            current_dir.join("crates/pixi_bench/my-local-channel")
        };
        let local_channel_url = format!("file://{}", local_channel_dir.to_string_lossy());
        let channels = vec![NamedChannelOrUrl::Url(local_channel_url.parse().unwrap())];

        // Setup: Install the package (not timed)
        project
            .manifest
            .add_environment(&env_name, Some(channels))?;
        let package_spec = "matplotlib==1.0.0";
        let global_spec = GlobalSpec::try_from_str(package_spec, project.global_channel_config())?;
        project.manifest.add_dependency(&env_name, &global_spec)?;
        let _ = project.install_environment(&env_name).await?;

        // Measure: Only the uninstall operation
        println!("⏱️ Timing: pixi global uninstall 1 packages");
        let start = Instant::now();
        let _ = project.remove_environment(&env_name).await?;
        let duration = start.elapsed();
        println!(
            "✅ Global uninstall completed in {:.2}s",
            duration.as_secs_f64()
        );

        Ok(duration)
    }

    /// Install and uninstall a large package (only uninstall is timed)
    async fn install_and_uninstall_large(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        // Set environment variables once for both operations
        for (key, value) in self.get_env_vars() {
            std::env::set_var(key, value);
        }

        // Create a single project instance for both operations
        let mut project = Project::discover_or_create().await?;
        let env_name = EnvironmentName::from_str("bench_jupyter")?;

        // Use local channel
        let current_dir = std::env::current_dir().unwrap_or_default();
        let local_channel_dir = if current_dir.ends_with("pixi_bench") {
            current_dir.join("my-local-channel")
        } else {
            current_dir.join("crates/pixi_bench/my-local-channel")
        };
        let local_channel_url = format!("file://{}", local_channel_dir.to_string_lossy());
        let channels = vec![NamedChannelOrUrl::Url(local_channel_url.parse().unwrap())];

        // Setup: Install the package (not timed)
        project
            .manifest
            .add_environment(&env_name, Some(channels))?;
        let package_spec = "jupyter==1.0.0";
        let global_spec = GlobalSpec::try_from_str(package_spec, project.global_channel_config())?;
        project.manifest.add_dependency(&env_name, &global_spec)?;
        let _ = project.install_environment(&env_name).await?;

        // Measure: Only the uninstall operation
        println!("⏱️ Timing: pixi global uninstall 1 packages");
        let start = Instant::now();
        let _ = project.remove_environment(&env_name).await?;
        let duration = start.elapsed();
        println!(
            "✅ Global uninstall completed in {:.2}s",
            duration.as_secs_f64()
        );

        Ok(duration)
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

fn bench_single_package(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("single_package_global_install");
    group.measurement_time(Duration::from_secs(60)); // Allow 1 minute for measurements
    group.sample_size(10); // Reduce sample size for long operations
    group.warm_up_time(Duration::from_secs(5)); // Warm up time

    // Cold cache benchmark - always creates new isolated environment
    group.bench_function("cold_cache_single", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .install_single_small()
                .await
                .expect("Failed to time pixi global install");
            black_box(duration)
        })
    });

    // Warm cache benchmark - reuses shared cache
    group.bench_function("warm_cache_single", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_single_small()
                .await
                .expect("Failed to time pixi global install");
            black_box(duration)
        })
    });
}

fn bench_multiple_packages(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("multiple_packages_global_install");
    group.measurement_time(Duration::from_secs(90)); // 1.5 minutes
    group.sample_size(10); // Minimum required samples
    group.warm_up_time(Duration::from_secs(10));

    // Cold cache benchmark
    group.bench_function("cold_cache_multiple", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .install_multiple_small()
                .await
                .expect("Failed to time pixi global install");
            black_box(duration)
        })
    });

    // Warm cache benchmark
    group.bench_function("warm_cache_multiple", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_multiple_small()
                .await
                .expect("Failed to time pixi global install");
            black_box(duration)
        })
    });
}

fn bench_package_sizes(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("package_sizes_global_install");
    group.measurement_time(Duration::from_secs(120)); // 2 minutes
    group.sample_size(10); // Minimum required samples
    group.warm_up_time(Duration::from_secs(15));

    // Medium package benchmark
    group.bench_function("medium_package", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_medium()
                .await
                .expect("Failed to time pixi global install");
            black_box(duration)
        })
    });

    // Large package benchmark
    group.bench_function("large_package", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_large()
                .await
                .expect("Failed to time pixi global install");
            black_box(duration)
        })
    });
}

fn bench_special_scenarios(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("special_scenarios_global_install");
    group.measurement_time(Duration::from_secs(90)); // 1.5 minutes
    group.sample_size(10); // Minimum required samples
    group.warm_up_time(Duration::from_secs(10));

    // Force reinstall benchmark
    group.bench_function("force_reinstall", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_with_force_reinstall()
                .await
                .expect("Failed to time pixi global install with force reinstall");
            black_box(duration)
        })
    });

    // Platform-specific install benchmark
    group.bench_function("platform_specific", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_with_platform()
                .await
                .expect("Failed to time pixi global install with platform");
            black_box(duration)
        })
    });

    // Custom channel benchmark
    group.bench_function("custom_channel", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_with_custom_channel()
                .await
                .expect("Failed to time pixi global install with custom channel");
            black_box(duration)
        })
    });
}

fn bench_single_package_uninstall(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("single_package_global_uninstall");
    group.measurement_time(Duration::from_secs(60)); // Allow 1 minute for measurements
    group.sample_size(10); // Reduce sample size for long operations
    group.warm_up_time(Duration::from_secs(5)); // Warm up time

    // Uninstall single package benchmark
    group.bench_function("uninstall_single", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            // Install and uninstall (only uninstall is timed)
            let duration = env
                .install_and_uninstall_single_small()
                .await
                .expect("Failed to time pixi global uninstall");
            black_box(duration)
        })
    });
}

fn bench_multiple_packages_uninstall(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("multiple_packages_global_uninstall");
    group.measurement_time(Duration::from_secs(90)); // 1.5 minutes
    group.sample_size(10); // Minimum required samples
    group.warm_up_time(Duration::from_secs(10));

    // Uninstall multiple packages benchmark
    group.bench_function("uninstall_multiple", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            // Install and uninstall (only uninstall is timed)
            let duration = env
                .install_and_uninstall_multiple_small()
                .await
                .expect("Failed to time pixi global uninstall");
            black_box(duration)
        })
    });
}

fn bench_package_sizes_uninstall(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("package_sizes_global_uninstall");
    group.measurement_time(Duration::from_secs(120)); // 2 minutes
    group.sample_size(10); // Minimum required samples
    group.warm_up_time(Duration::from_secs(15));

    // Medium package uninstall benchmark
    group.bench_function("uninstall_medium_package", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            // Install and uninstall (only uninstall is timed)
            let duration = env
                .install_and_uninstall_medium()
                .await
                .expect("Failed to time pixi global uninstall");
            black_box(duration)
        })
    });

    // Large package uninstall benchmark
    group.bench_function("uninstall_large_package", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            // Install and uninstall (only uninstall is timed)
            let duration = env
                .install_and_uninstall_large()
                .await
                .expect("Failed to time pixi global uninstall");
            black_box(duration)
        })
    });
}

criterion_group!(
    benches,
    bench_single_package,
    bench_multiple_packages,
    bench_package_sizes,
    bench_special_scenarios,
    bench_single_package_uninstall,
    bench_multiple_packages_uninstall,
    bench_package_sizes_uninstall
);
criterion_main!(benches);
