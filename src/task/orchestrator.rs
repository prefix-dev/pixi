use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

use crate::task::{
    reload::FileWatchError,
    task_graph::{TaskGraph, TaskId},
    ExecutableTask, TaskExecutionError,
};

/// Manages the lifecycle of long-running tasks with dependencies
pub struct TaskOrchestrator {
    /// The task dependency graph
    task_graph: Arc<TaskGraph<'static>>,
    
    /// Currently running tasks and their cancellation channels
    running_tasks: Arc<Mutex<HashMap<TaskId, mpsc::Sender<()>>>>,
    
    /// File paths being watched per task
    task_file_dependencies: Arc<Mutex<HashMap<TaskId, Vec<PathBuf>>>>,
    
    /// Flag to indicate if orchestrator is shutting down
    is_shutting_down: Arc<Mutex<bool>>,
    
    /// Debounce time for file change events
    debounce_time: Duration,
}

impl TaskOrchestrator {
    /// Create a new task orchestrator from a task graph
    pub fn new(task_graph: TaskGraph<'static>) -> Self {
        Self {
            task_graph: Arc::new(task_graph),
            running_tasks: Arc::new(Mutex::new(HashMap::new())),
            task_file_dependencies: Arc::new(Mutex::new(HashMap::new())),
            is_shutting_down: Arc::new(Mutex::new(false)),
            debounce_time: Duration::from_millis(300),
        }
    }
    
    /// Set the debounce time for file change events
    pub fn with_debounce_time(mut self, duration: Duration) -> Self {
        self.debounce_time = duration;
        self
    }
    
    /// Start all tasks in topological order
    pub async fn start_all_tasks(&self, command_env: &HashMap<String, String>) -> Result<(), TaskExecutionError> {
        // Get topological order to ensure dependencies are started first
        let task_ids = self.task_graph.topological_order();
        
        for task_id in task_ids {
            self.start_task(task_id, command_env).await?;
        }
        
        Ok(())
    }
    
    /// Start a single task and track its cancellation channel
    pub async fn start_task(&self, task_id: TaskId, command_env: &HashMap<String, String>) -> Result<(), TaskExecutionError> {
        let executable_task = ExecutableTask::from_task_graph(&self.task_graph, task_id);
        
        // Store watched files for this task
        self.register_watched_files(&executable_task, task_id).await?;
        
        // Create cancel channel for the task
        let (cancel_tx, mut cancel_rx) = mpsc::channel::<()>(1);
        
        // Store the cancellation sender
        {
            let mut tasks = self.running_tasks.lock().await;
            
            // If the task is already running, cancel it first
            if let Some(existing_tx) = tasks.remove(&task_id) {
                info!("Cancelling existing task {} before restart", task_id.index());
                let _ = existing_tx.send(()).await;
                // Small delay to allow the task to clean up
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            
            tasks.insert(task_id, cancel_tx.clone());
        }
        
        // Execute in a separate OS thread to avoid tokio Send issues with Rc values
        let task_id_copy = task_id;
        let command_env_clone = command_env.clone();
        let running_tasks_clone = self.running_tasks.clone();
        let executable_task_clone = executable_task.clone();
        
        // Spawn on a standard thread, not tokio thread
        std::thread::spawn(move || {
            // Create a new runtime for this thread
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            
            rt.block_on(async {
                info!("Starting task {}", task_id_copy.index());
                
                // Prepare script and working directory
                let script = match executable_task_clone.as_deno_script() {
                    Ok(Some(s)) => s,
                    Ok(None) => {
                        // Nothing to execute
                        let mut tasks = running_tasks_clone.lock().await;
                        tasks.remove(&task_id_copy);
                        return;
                    },
                    Err(e) => {
                        warn!("Error parsing script for task {}: {:?}", task_id_copy.index(), e);
                        let mut tasks = running_tasks_clone.lock().await;
                        tasks.remove(&task_id_copy);
                        return;
                    }
                };
                
                let cwd = match executable_task_clone.working_directory() {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("Invalid working directory for task {}: {:?}", task_id_copy.index(), e);
                        let mut tasks = running_tasks_clone.lock().await;
                        tasks.remove(&task_id_copy);
                        return;
                    }
                };
                
                // Execute with cancellation
                let status_code = tokio::select! {
                    code = deno_task_shell::execute(
                        script,
                        command_env_clone.clone(),
                        &cwd,
                        Default::default(),
                        Default::default(),
                    ) => code,
                    
                    _ = cancel_rx.recv() => {
                        // Task was cancelled
                        info!("Task {} was cancelled", task_id_copy.index());
                        0
                    }
                };
                
                // Task finished
                let mut tasks = running_tasks_clone.lock().await;
                tasks.remove(&task_id_copy);
                
                if status_code != 0 {
                    warn!("Task {} exited with error code: {}", task_id_copy.index(), status_code);
                } else {
                    info!("Task {} completed", task_id_copy.index());
                }
            });
        });
        
        Ok(())
    }
    
    /// Register the files watched by a task
    async fn register_watched_files(&self, task: &ExecutableTask<'_>, task_id: TaskId) -> Result<(), TaskExecutionError> {
        let mut watched_files = Vec::new();
        
        // Collect files from task inputs
        if let Some(execute) = task.task().as_execute() {
            if let Some(inputs) = &execute.inputs {
                let root_path = task.project().root();
                
                // Convert inputs to absolute paths
                for input in inputs {
                    watched_files.push(root_path.join(input));
                }
            }
        }
        
        // Store the watched files for this task
        if !watched_files.is_empty() {
            let mut task_deps = self.task_file_dependencies.lock().await;
            task_deps.insert(task_id, watched_files);
        }
        
        Ok(())
    }
    
    /// Stop all tasks in reverse topological order
    pub async fn stop_all_tasks(&self) {
        info!("Stopping all tasks in reverse dependency order");
        
        // Set shutting down flag
        {
            let mut is_shutting_down = self.is_shutting_down.lock().await;
            *is_shutting_down = true;
        }
        
        // Get reverse topological order to ensure dependents are stopped before dependencies
        let task_ids = self.reverse_topological_order();
        
        for task_id in task_ids {
            self.stop_task(task_id).await;
        }
    }
    
    /// Stop a single task
    pub async fn stop_task(&self, task_id: TaskId) {
        let mut tasks = self.running_tasks.lock().await;
        
        if let Some(cancel_tx) = tasks.remove(&task_id) {
            info!("Stopping task {}", task_id.index());
            
            // Send cancellation signal
            let _ = cancel_tx.send(()).await;
        }
    }
    
    /// Find tasks affected by changes to specific files
    pub async fn find_affected_tasks(&self, changed_files: &[PathBuf]) -> HashSet<TaskId> {
        let mut affected_tasks = HashSet::new();
        
        // First, find directly affected tasks
        {
            let task_deps = self.task_file_dependencies.lock().await;
            
            for (task_id, watched_files) in task_deps.iter() {
                for changed_file in changed_files {
                    // Check if any watched file matches the changed file
                    for watched_file in watched_files {
                        if is_file_affected(watched_file, changed_file) {
                            affected_tasks.insert(*task_id);
                            break;
                        }
                    }
                    
                    if affected_tasks.contains(task_id) {
                        break;
                    }
                }
            }
        }
        
        // Then find tasks that depend on affected tasks (transitive dependencies)
        let mut additional_affected = HashSet::new();
        
        for task_id in &affected_tasks {
            self.add_dependent_tasks(*task_id, &mut additional_affected);
        }
        
        // Combine directly affected and dependent tasks
        affected_tasks.extend(additional_affected);
        
        affected_tasks
    }
    
    /// Add all tasks that depend on the given task (recursively)
    fn add_dependent_tasks(&self, task_id: TaskId, affected: &mut HashSet<TaskId>) {
        for i in 0..self.task_graph.nodes_len() {
            let dependent_id = TaskId::new(i);
            
            // Skip tasks that are already in the affected set
            if affected.contains(&dependent_id) {
                continue;
            }
            
            // Check if this task depends on the affected task
            let node = &self.task_graph[dependent_id];
            if node.dependencies.contains(&task_id) {
                affected.insert(dependent_id);
                
                // Recursively find tasks that depend on this one
                self.add_dependent_tasks(dependent_id, affected);
            }
        }
    }
    
    /// Restart tasks affected by file changes
    pub async fn restart_affected_tasks(
        &self, 
        changed_files: &[PathBuf], 
        command_env: &HashMap<String, String>
    ) -> Result<(), TaskExecutionError> {
        // Find tasks affected by file changes
        let affected_tasks = self.find_affected_tasks(changed_files).await;
        
        if affected_tasks.is_empty() {
            return Ok(());
        }
        
        info!("Restarting {} tasks affected by file changes", affected_tasks.len());
        
        // First stop affected tasks in reverse dependency order
        let rev_order = self.reverse_topological_order();
        let rev_affected: Vec<_> = rev_order.into_iter()
            .filter(|id| affected_tasks.contains(id))
            .collect();
            
        // Stop tasks in reverse order
        for task_id in rev_affected {
            self.stop_task(task_id).await;
        }
        
        // Then restart tasks in dependency order
        let topo_order = self.task_graph.topological_order();
        let ordered_affected: Vec<_> = topo_order.into_iter()
            .filter(|id| affected_tasks.contains(id))
            .collect();
            
        // Start tasks in topological order
        for task_id in ordered_affected {
            self.start_task(task_id, command_env).await?;
        }
        
        Ok(())
    }
    
    /// Get a reverse topological ordering of tasks (dependents before dependencies)
    fn reverse_topological_order(&self) -> Vec<TaskId> {
        // Get normal topological order first
        let mut topo_order = self.task_graph.topological_order();
        
        // Reverse it to get dependents before dependencies
        topo_order.reverse();
        
        topo_order
    }
    
    /// Check if the orchestrator is shutting down
    pub async fn is_shutting_down(&self) -> bool {
        let is_shutting_down = self.is_shutting_down.lock().await;
        *is_shutting_down
    }
    
    /// Collect all files watched by any task
    pub async fn collect_all_watched_files(&self) -> Vec<PathBuf> {
        let mut result = Vec::new();
        let task_deps = self.task_file_dependencies.lock().await;
        
        for files in task_deps.values() {
            result.extend(files.iter().cloned());
        }
        
        result
    }
}

/// Check if a watched file is affected by a changed file
fn is_file_affected(watched_file: &PathBuf, changed_file: &PathBuf) -> bool {
    // Direct match
    if watched_file == changed_file {
        return true;
    }
    
    // Check if watched file is a glob pattern
    let watched_str = watched_file.to_string_lossy();
    if watched_str.contains('*') || watched_str.contains('?') || watched_str.contains('[') {
        // For now, just check if the changed file is in the same directory
        if let Some(watched_parent) = watched_file.parent() {
            if let Some(changed_parent) = changed_file.parent() {
                return watched_parent == changed_parent;
            }
        }
    }
    
    // Check if watched file is a directory containing the changed file
    if watched_file.is_dir() && changed_file.starts_with(watched_file) {
        return true;
    }
    
    false
}

/// Main function for watching and executing tasks
pub async fn watch_and_execute_tasks(
    task_graph: TaskGraph<'static>,
    command_env: &HashMap<String, String>,
) -> Result<(), FileWatchError> {
    // Create task orchestrator
    let orchestrator = Arc::new(TaskOrchestrator::new(task_graph));
    
    // Start all tasks initially
    orchestrator.start_all_tasks(command_env).await
        .map_err(|e| FileWatchError::TaskExecutionError(e.to_string()))?;
    
    // Set up signal handler for Ctrl+C
    let orchestrator_clone = orchestrator.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.unwrap();
        orchestrator_clone.stop_all_tasks().await;
        std::process::exit(0);
    });
    
    // Collect all watched files from all tasks
    let watched_files = orchestrator.collect_all_watched_files().await;
    
    // Create file watcher
    use crate::task::reload::FileWatcher;
    let mut watcher = FileWatcher::new(&watched_files)?;
    
    // Track when we last processed a file change event
    let mut last_event_time = std::time::Instant::now();
    
    // File watch event loop
    while let Some(event) = watcher.next_event().await {
        // Check if orchestrator is shutting down
        if orchestrator.is_shutting_down().await {
            break;
        }
        
        match event {
            Ok(event) => {
                // Process file change event
                match event.kind {
                    notify::event::EventKind::Create(_) |
                    notify::event::EventKind::Modify(_) |
                    notify::event::EventKind::Remove(_) => {
                        // Debounce handling
                        let now = std::time::Instant::now();
                        if now.duration_since(last_event_time) < orchestrator.debounce_time {
                            continue;
                        }
                        
                        last_event_time = now;
                        
                        // Extract changed files from the event
                        let mut changed_files = Vec::new();
                        for path in event.paths {
                            changed_files.push(path);
                        }
                        
                        if !changed_files.is_empty() {
                            // Restart only affected tasks
                            if let Err(e) = orchestrator.restart_affected_tasks(&changed_files, command_env).await {
                                warn!("Error restarting tasks: {}", e);
                            }
                        }
                    }
                    _ => continue, // Ignore other event types
                }
            }
            Err(e) => {
                warn!("Error watching files: {}", e);
                break;
            }
        }
    }
    
    // At this point, something has caused the watcher to exit
    orchestrator.stop_all_tasks().await;
    
    Ok(())
} 