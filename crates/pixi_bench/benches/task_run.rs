use criterion::{black_box, criterion_group, criterion_main, Criterion};
use fs_err as fs;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tempfile::TempDir;

// Pixi crate imports for direct API usage
use pixi_cli::run;
use pixi_config::ConfigCli;

// Single global runtime for all benchmarks
static RUNTIME: Lazy<tokio::runtime::Runtime> =
    Lazy::new(|| tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime"));

/// Create an isolated pixi environment for task runner testing
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

    /// Create pixi project with tasks
    fn create_pixi_project_with_tasks(
        &mut self,
        _packages: &[&str],
        task_type: TaskType,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::fs::File;
        use std::io::Write;

        // Create a minimal pixi.toml without external dependencies to avoid platform issues
        let mut pixi_toml = r#"[project]
name = "task-benchmark-project"
version = "0.1.0"
description = "Benchmark project for pixi task runner testing"
channels = ["conda-forge"]
platforms = ["osx-arm64", "linux-64", "win-64"]

[dependencies]
# No external dependencies to avoid platform resolution issues

"#
        .to_string();

        // Add tasks based on the task type
        pixi_toml.push_str("\n[tasks]\n");
        match task_type {
            TaskType::Simple => {
                pixi_toml.push_str(
                    r#"simple = "echo 'Hello from simple task'"
simple-with-args = "echo 'Task with args:' $@"
"#,
                );
            }
            TaskType::Complex => {
                pixi_toml.push_str(r#"complex = "echo 'Starting complex task' && sleep 0.1 && echo 'Complex task completed'"
multi-step = "echo 'Step 1: Preparation' && echo 'Step 2: Processing' && echo 'Step 3: Cleanup'"
"#);
            }
            TaskType::WithDependencies => {
                pixi_toml.push_str(
                    r#"prepare = "echo 'Preparing...'"
build = { cmd = "echo 'Building...'", depends-on = ["prepare"] }
test = { cmd = "echo 'Testing...'", depends-on = ["build"] }
deploy = { cmd = "echo 'Deploying...'", depends-on = ["test"] }
"#,
                );
            }
            TaskType::Python => {
                pixi_toml.push_str(
                    r#"shell-simple = "echo 'Hello from shell task'"
shell-version = "echo 'Shell version check'"
shell-script = "echo 'Running shell script' && date"
"#,
                );
            }
        }

        let mut file = File::create(self.project_dir.join("pixi.toml"))?;
        file.write_all(pixi_toml.as_bytes())?;

        self.project_created = true;
        Ok(())
    }

    /// Run a pixi task and measure execution time
    async fn run_pixi_task(
        &mut self,
        packages: &[&str],
        task_type: TaskType,
        task_name: &str,
        task_args: Vec<String>,
    ) -> Result<Duration, Box<dyn std::error::Error>> {
        // Create project if not already created
        if !self.project_created {
            self.create_pixi_project_with_tasks(packages, task_type)?;
        }

        // Set environment variables for pixi
        for (key, value) in self.get_env_vars() {
            std::env::set_var(key, value);
        }

        // Change to project directory
        let original_dir = std::env::current_dir()?;
        std::env::set_current_dir(&self.project_dir)?;

        let start = Instant::now();

        // Create run arguments
        let mut task_cmd = vec![task_name.to_string()];
        task_cmd.extend(task_args);

        let run_args = run::Args {
            task: task_cmd,
            workspace_config: pixi_cli::cli_config::WorkspaceConfig::default(),
            lock_and_install_config: pixi_cli::cli_config::LockAndInstallConfig::default(),
            config: ConfigCli::default(),
            activation_config: pixi_config::ConfigCliActivation::default(),
            environment: None,
            clean_env: false,
            skip_deps: false,
            dry_run: false,
            help: None,
            h: None,
        };

        // Execute pixi run directly
        let result = run::execute(run_args).await;

        // Restore original directory
        std::env::set_current_dir(original_dir)?;

        match result {
            Ok(_) => {
                let duration = start.elapsed();
                println!(
                    "✅ Task '{}' completed in {:.2}s",
                    task_name,
                    duration.as_secs_f64()
                );
                Ok(duration)
            }
            Err(e) => {
                println!("❌ Task '{}' failed: {}", task_name, e);
                Err(format!("Task '{}' failed: {}", task_name, e).into())
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum TaskType {
    Simple,
    Complex,
    WithDependencies,
    Python,
}

fn bench_simple_tasks(c: &mut Criterion) {
    let packages = [];

    let mut group = c.benchmark_group("simple_task_execution");
    group.measurement_time(Duration::from_secs(30));
    group.sample_size(15);
    group.warm_up_time(Duration::from_secs(5));

    // Simple echo task
    group.bench_function("simple_echo", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .run_pixi_task(&packages, TaskType::Simple, "simple", vec![])
                .await
                .expect("Failed to run simple task");
            black_box(duration)
        })
    });

    // Simple task with arguments
    group.bench_function("simple_with_args", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .run_pixi_task(
                    &packages,
                    TaskType::Simple,
                    "simple-with-args",
                    vec!["arg1".to_string(), "arg2".to_string()],
                )
                .await
                .expect("Failed to run simple task with args");
            black_box(duration)
        })
    });
}

fn bench_complex_tasks(c: &mut Criterion) {
    let packages = [];

    let mut group = c.benchmark_group("complex_task_execution");
    group.measurement_time(Duration::from_secs(45));
    group.sample_size(12);
    group.warm_up_time(Duration::from_secs(5));

    // Complex task with multiple commands
    group.bench_function("complex_multi_command", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .run_pixi_task(&packages, TaskType::Complex, "complex", vec![])
                .await
                .expect("Failed to run complex task");
            black_box(duration)
        })
    });

    // Multi-step task
    group.bench_function("multi_step_task", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .run_pixi_task(&packages, TaskType::Complex, "multi-step", vec![])
                .await
                .expect("Failed to run multi-step task");
            black_box(duration)
        })
    });
}

fn bench_dependency_tasks(c: &mut Criterion) {
    let packages = [];

    let mut group = c.benchmark_group("dependency_task_execution");
    group.measurement_time(Duration::from_secs(60));
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(5));

    // Task with dependencies (should run prepare -> build -> test -> deploy)
    group.bench_function("task_with_dependencies", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .run_pixi_task(&packages, TaskType::WithDependencies, "deploy", vec![])
                .await
                .expect("Failed to run task with dependencies");
            black_box(duration)
        })
    });

    // Single dependency task
    group.bench_function("single_dependency", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .run_pixi_task(&packages, TaskType::WithDependencies, "build", vec![])
                .await
                .expect("Failed to run task with single dependency");
            black_box(duration)
        })
    });
}

fn bench_python_tasks(c: &mut Criterion) {
    let packages = [];

    let mut group = c.benchmark_group("shell_task_execution");
    group.measurement_time(Duration::from_secs(30));
    group.sample_size(15);
    group.warm_up_time(Duration::from_secs(5));

    // Simple shell task
    group.bench_function("shell_simple", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .run_pixi_task(&packages, TaskType::Python, "shell-simple", vec![])
                .await
                .expect("Failed to run shell simple task");
            black_box(duration)
        })
    });

    // Shell version check
    group.bench_function("shell_version", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .run_pixi_task(&packages, TaskType::Python, "shell-version", vec![])
                .await
                .expect("Failed to run shell version task");
            black_box(duration)
        })
    });

    // Shell script execution
    group.bench_function("shell_script", |b| {
        b.to_async(&*RUNTIME).iter(|| async {
            let mut env = IsolatedPixiEnv::new().expect("Failed to create isolated environment");
            let duration = env
                .run_pixi_task(&packages, TaskType::Python, "shell-script", vec![])
                .await
                .expect("Failed to run shell script task");
            black_box(duration)
        })
    });
}

criterion_group!(
    benches,
    bench_simple_tasks,
    bench_complex_tasks,
    bench_dependency_tasks,
    bench_python_tasks
);
criterion_main!(benches);
