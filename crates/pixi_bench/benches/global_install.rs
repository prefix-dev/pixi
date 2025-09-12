use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fs_err as fs;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::TempDir;

// Pixi crate imports for direct API usage
use rattler_conda_types::{NamedChannelOrUrl, Platform};

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

    /// Run pixi global install and measure execution time
    fn pixi_global_install(
        &self,
        packages: &[&str],
        channels: Option<Vec<NamedChannelOrUrl>>,
        platform: Option<Platform>,
        force_reinstall: bool,
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        println!("⏱️ Timing: pixi global install {} packages", packages.len());

        let start = Instant::now();

        // Build command arguments for global install
        let mut args = vec!["global".to_string(), "install".to_string()];

        // Add packages
        for package in packages {
            args.push(package.to_string());
        }

        // Add channels if specified
        if let Some(channels) = channels {
            for channel in channels {
                args.push("--channel".to_string());
                args.push(channel.to_string());
            }
        }

        // Add platform if specified
        if let Some(platform) = platform {
            args.push("--platform".to_string());
            args.push(platform.to_string());
        }

        // Add force reinstall if specified
        if force_reinstall {
            args.push("--force-reinstall".to_string());
        }

        // Add no shortcuts for benchmarking
        args.push("--no-shortcuts".to_string());

        // Use system pixi binary to avoid permission issues
        let pixi_binary = "pixi";

        // Execute pixi global install as subprocess
        let mut cmd = Command::new(pixi_binary);
        cmd.args(&args);

        // Set environment variables
        for (key, value) in self.get_env_vars() {
            cmd.env(key, value);
        }

        let result = cmd.output();

        match result {
            Ok(output) => {
                let duration = start.elapsed();
                if output.status.success() {
                    println!(
                        "✅ Global install completed in {:.2}s",
                        duration.as_secs_f64()
                    );
                    Ok(duration)
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    println!("❌ pixi global install failed: {}", stderr);
                    Err(format!("pixi global install failed: {}", stderr).into())
                }
            }
            Err(e) => {
                println!("❌ pixi global install failed to execute: {}", e);
                Err(format!("pixi global install failed to execute: {}", e).into())
            }
        }
    }

    /// Install a single small package
    fn install_single_small(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.pixi_global_install(&["ripgrep"], None, None, false)
    }

    /// Install multiple small packages
    fn install_multiple_small(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.pixi_global_install(&["ripgrep", "bat", "fd-find"], None, None, false)
    }

    /// Install a medium-sized package
    fn install_medium(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.pixi_global_install(&["starship"], None, None, false)
    }

    /// Install a large package
    fn install_large(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        self.pixi_global_install(&["jupyter"], None, None, false)
    }

    /// Install with force reinstall
    fn install_with_force_reinstall(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        // First install
        let _ = self.pixi_global_install(&["ripgrep"], None, None, false)?;
        // Then force reinstall
        self.pixi_global_install(&["ripgrep"], None, None, true)
    }

    /// Install with specific platform
    fn install_with_platform(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        let platform = Platform::current();
        self.pixi_global_install(&["bat"], None, Some(platform), false)
    }

    /// Install with custom channel
    fn install_with_custom_channel(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        let channels = vec![
            NamedChannelOrUrl::Name("conda-forge".to_string()),
            NamedChannelOrUrl::Name("bioconda".to_string()),
        ];
        self.pixi_global_install(&["samtools"], Some(channels), None, false)
    }

    /// Run pixi global uninstall and measure execution time
    fn pixi_global_uninstall(
        &self,
        packages: &[&str],
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        println!(
            "⏱️ Timing: pixi global uninstall {} packages",
            packages.len()
        );

        let start = Instant::now();

        // Build command arguments for global uninstall
        let mut args = vec!["global".to_string(), "uninstall".to_string()];

        // Add packages
        for package in packages {
            args.push(package.to_string());
        }

        // Use system pixi binary to avoid permission issues
        let pixi_binary = "pixi";

        // Execute pixi global uninstall as subprocess
        let mut cmd = Command::new(pixi_binary);
        cmd.args(&args);

        // Set environment variables
        for (key, value) in self.get_env_vars() {
            cmd.env(key, value);
        }

        let result = cmd.output();

        match result {
            Ok(output) => {
                let duration = start.elapsed();
                if output.status.success() {
                    println!(
                        "✅ Global uninstall completed in {:.2}s",
                        duration.as_secs_f64()
                    );
                    Ok(duration)
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    println!("❌ pixi global uninstall failed: {}", stderr);
                    Err(format!("pixi global uninstall failed: {}", stderr).into())
                }
            }
            Err(e) => {
                println!("❌ pixi global uninstall failed to execute: {}", e);
                Err(format!("pixi global uninstall failed to execute: {}", e).into())
            }
        }
    }

    /// Uninstall a single small package
    fn uninstall_single_small(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        // First install the package
        let _ = self.pixi_global_install(&["ripgrep"], None, None, false)?;
        // Then uninstall it
        self.pixi_global_uninstall(&["ripgrep"])
    }

    /// Uninstall multiple small packages
    fn uninstall_multiple_small(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        // First install the packages
        let _ = self.pixi_global_install(&["ripgrep", "bat", "fd-find"], None, None, false)?;
        // Then uninstall them
        self.pixi_global_uninstall(&["ripgrep", "bat", "fd-find"])
    }

    /// Uninstall a medium-sized package
    fn uninstall_medium(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        // First install the package
        let _ = self.pixi_global_install(&["starship"], None, None, false)?;
        // Then uninstall it
        self.pixi_global_uninstall(&["starship"])
    }

    /// Uninstall a large package
    fn uninstall_large(&self) -> Result<Duration, Box<dyn std::error::Error>> {
        // First install the package
        let _ = self.pixi_global_install(&["jupyter"], None, None, false)?;
        // Then uninstall it
        self.pixi_global_uninstall(&["jupyter"])
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
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .install_single_small()
                .expect("Failed to time pixi global install");
            black_box(duration)
        })
    });

    // Warm cache benchmark - reuses shared cache
    group.bench_function("warm_cache_single", |b| {
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_single_small()
                .expect("Failed to time pixi global install");
            black_box(duration)
        })
    });
}

fn bench_multiple_packages(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("multiple_packages_global_install");
    group.measurement_time(Duration::from_secs(90)); // 1.5 minutes
    group.sample_size(8); // Fewer samples for multiple packages
    group.warm_up_time(Duration::from_secs(10));

    // Cold cache benchmark
    group.bench_function("cold_cache_multiple", |b| {
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .install_multiple_small()
                .expect("Failed to time pixi global install");
            black_box(duration)
        })
    });

    // Warm cache benchmark
    group.bench_function("warm_cache_multiple", |b| {
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_multiple_small()
                .expect("Failed to time pixi global install");
            black_box(duration)
        })
    });
}

fn bench_package_sizes(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("package_sizes_global_install");
    group.measurement_time(Duration::from_secs(120)); // 2 minutes
    group.sample_size(5); // Very few samples for large packages
    group.warm_up_time(Duration::from_secs(15));

    // Medium package benchmark
    group.bench_function("medium_package", |b| {
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_medium()
                .expect("Failed to time pixi global install");
            black_box(duration)
        })
    });

    // Large package benchmark
    group.bench_function("large_package", |b| {
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_large()
                .expect("Failed to time pixi global install");
            black_box(duration)
        })
    });
}

fn bench_special_scenarios(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("special_scenarios_global_install");
    group.measurement_time(Duration::from_secs(90)); // 1.5 minutes
    group.sample_size(6); // Moderate samples for special scenarios
    group.warm_up_time(Duration::from_secs(10));

    // Force reinstall benchmark
    group.bench_function("force_reinstall", |b| {
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_with_force_reinstall()
                .expect("Failed to time pixi global install with force reinstall");
            black_box(duration)
        })
    });

    // Platform-specific install benchmark
    group.bench_function("platform_specific", |b| {
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_with_platform()
                .expect("Failed to time pixi global install with platform");
            black_box(duration)
        })
    });

    // Custom channel benchmark
    group.bench_function("custom_channel", |b| {
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .install_with_custom_channel()
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
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .uninstall_single_small()
                .expect("Failed to time pixi global uninstall");
            black_box(duration)
        })
    });
}

fn bench_multiple_packages_uninstall(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("multiple_packages_global_uninstall");
    group.measurement_time(Duration::from_secs(90)); // 1.5 minutes
    group.sample_size(8); // Fewer samples for multiple packages
    group.warm_up_time(Duration::from_secs(10));

    // Uninstall multiple packages benchmark
    group.bench_function("uninstall_multiple", |b| {
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .uninstall_multiple_small()
                .expect("Failed to time pixi global uninstall");
            black_box(duration)
        })
    });
}

fn bench_package_sizes_uninstall(c: &mut Criterion) {
    let shared_cache = SharedCache::new().expect("Failed to create shared cache");
    let mut group = c.benchmark_group("package_sizes_global_uninstall");
    group.measurement_time(Duration::from_secs(120)); // 2 minutes
    group.sample_size(5); // Very few samples for large packages
    group.warm_up_time(Duration::from_secs(15));

    // Medium package uninstall benchmark
    group.bench_function("uninstall_medium_package", |b| {
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .uninstall_medium()
                .expect("Failed to time pixi global uninstall");
            black_box(duration)
        })
    });

    // Large package uninstall benchmark
    group.bench_function("uninstall_large_package", |b| {
        b.iter(|| {
            let env = IsolatedPixiGlobalEnv::new_with_shared_cache(&shared_cache.cache_dir)
                .expect("Failed to create environment with shared cache");
            let duration = env
                .uninstall_large()
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
