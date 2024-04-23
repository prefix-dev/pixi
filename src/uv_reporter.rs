use crate::progress::{self, ProgressBarMessageFormatter, ScopedTask};
use distribution_types::{BuildableSource, CachedDist, Name, VersionOrUrl};
use indicatif::ProgressBar;
use itertools::Itertools;
use std::{collections::HashMap, sync::Arc, time::Duration};
use uv_normalize::PackageName;

fn create_progress(length: u64, message: &'static str) -> ProgressBar {
    // Construct a progress bar to provide some indication on what is currently downloading.
    //  For instance if we could also show at what speed the downloads are progressing or the total
    //  size of the downloads that would really help the user I think.
    let pb = progress::global_multi_progress().add(ProgressBar::new(length));
    pb.set_style(progress::default_progress_style());
    pb.set_prefix(message);
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

pub struct UvReporterOptions {
    length: Option<u64>,
    top_level_message: &'static str,
    progress_bar: Option<ProgressBar>,
    starting_tasks: Vec<String>,
    capacity: Option<usize>,
}

impl UvReporterOptions {
    pub fn new() -> Self {
        Self {
            length: None,
            top_level_message: "",
            progress_bar: None,
            starting_tasks: Vec::new(),
            capacity: None,
        }
    }

    pub fn with_length(mut self, length: u64) -> Self {
        self.length = Some(length);
        self
    }

    pub fn with_top_level_message(mut self, message: &'static str) -> Self {
        self.top_level_message = message;
        self
    }

    pub fn with_existing(mut self, progress_bar: ProgressBar) -> Self {
        self.progress_bar = Some(progress_bar);
        self
    }

    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = Some(capacity);
        self
    }

    pub fn with_starting_tasks(mut self, tasks: impl Iterator<Item = String>) -> Self {
        self.starting_tasks = tasks.collect_vec();
        self
    }
}

/// Reports on download progress.
pub struct UvReporter {
    pb: ProgressBar,
    fmt: ProgressBarMessageFormatter,
    scoped_tasks: Arc<std::sync::Mutex<Vec<Option<ScopedTask>>>>,
    name_to_id: HashMap<String, usize>,
}

impl UvReporter {
    /// Create a new instance that will report on the progress the given uv reporter
    /// This uses a set size and message
    pub fn new(options: UvReporterOptions) -> Self {
        // Use a new progress bar if none was provided.
        let pb = if let Some(pb) = options.progress_bar {
            pb
        } else {
            create_progress(
                options.length.unwrap_or_default(),
                options.top_level_message,
            )
        };

        // Create the formatter
        let fmt = ProgressBarMessageFormatter::new_with_capacity(
            pb.clone(),
            options.capacity.unwrap_or(20),
        );

        let mut name_to_id = std::collections::HashMap::new();
        let mut starting_tasks = vec![];
        // Add the starting tasks
        for task in options.starting_tasks {
            let scoped_task = fmt.start_sync(task.clone());
            starting_tasks.push(Some(scoped_task));
            name_to_id.insert(task, starting_tasks.len() - 1);
        }

        Self {
            pb,
            fmt,
            scoped_tasks: Arc::new(std::sync::Mutex::new(starting_tasks)),
            name_to_id,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<Vec<Option<ScopedTask>>> {
        self.scoped_tasks.lock().expect("progress lock poison")
    }

    pub fn start_sync(&self, message: String) -> usize {
        let task = self.fmt.start_sync(message);
        let mut lock = self.lock();
        lock.push(Some(task));
        lock.len() - 1
    }

    pub fn finish(&self, id: usize) {
        let mut lock = self.lock();
        let len = lock.len();
        let task = lock
            .get_mut(id)
            .unwrap_or_else(|| panic!("progress bar error idx ({id}) > {len}"))
            .take();
        if let Some(task) = task {
            task.finish_sync();
        }
    }

    pub fn finish_all(&self) {
        self.pb.finish_and_clear()
    }

    pub fn increment_progress(&self) {
        self.pb.inc(1);
    }
}

impl uv_installer::DownloadReporter for UvReporter {
    fn on_progress(&self, dist: &CachedDist) {
        if let Some(id) = self.name_to_id.get(&format!("{}", dist.name())) {
            self.finish(*id);
        }
        self.increment_progress();
    }

    fn on_complete(&self) {
        self.finish_all();
    }

    fn on_build_start(&self, dist: &BuildableSource) -> usize {
        self.start_sync(format!("building {}", dist))
    }

    fn on_build_complete(&self, _dist: &BuildableSource, id: usize) {
        self.finish(id);
    }

    fn on_editable_build_start(&self, dist: &distribution_types::LocalEditable) -> usize {
        let path = dist.path.file_name();
        if let Some(path) = path {
            self.start_sync(format!(
                "building editable source {}",
                path.to_string_lossy()
            ))
        } else {
            self.start_sync("building editable source".to_string())
        }
    }

    fn on_editable_build_complete(&self, _dist: &distribution_types::LocalEditable, id: usize) {
        self.finish(id);
    }

    fn on_checkout_start(&self, url: &url::Url, _rev: &str) -> usize {
        self.start_sync(format!("cloning {}", url))
    }

    fn on_checkout_complete(&self, _url: &url::Url, _rev: &str, index: usize) {
        self.finish(index);
    }
}

impl uv_installer::InstallReporter for UvReporter {
    fn on_install_progress(&self, wheel: &CachedDist) {
        if let Some(id) = self.name_to_id.get(&format!("{}", wheel.name())) {
            self.finish(*id);
        }
        self.increment_progress();
    }

    fn on_install_complete(&self) {
        self.finish_all()
    }
}

impl uv_resolver::ResolverReporter for UvReporter {
    fn on_progress(&self, name: &PackageName, version: &VersionOrUrl) {
        self.pb
            .set_message(format!("resolving {}{}", name, version));
    }

    fn on_build_start(&self, dist: &BuildableSource) -> usize {
        self.start_sync(format!("building {}", dist,))
    }

    fn on_build_complete(&self, _dist: &BuildableSource, id: usize) {
        self.finish(id);
    }

    fn on_checkout_start(&self, url: &url::Url, _rev: &str) -> usize {
        self.start_sync(format!("cloning {}", url))
    }

    fn on_checkout_complete(&self, _url: &url::Url, _rev: &str, index: usize) {
        self.finish(index);
    }

    fn on_complete(&self) {
        self.finish_all()
    }
}
