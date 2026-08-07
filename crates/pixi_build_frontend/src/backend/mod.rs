use std::fmt::{Debug, Formatter};

use rattler_conda_types::VersionWithSource;

pub mod in_memory;

use in_memory::InMemoryBackend;
use pixi_build_types::{
    BackendCapabilities, PixiBuildApiVersion,
    procedures::{
        conda_build_v1::{CondaBuildV1Params, CondaBuildV1Result},
        conda_outputs::{CondaOutputsParams, CondaOutputsResult},
        log::{LogLevel, LogParams},
    },
};

mod output;

use crate::json_rpc::CommunicationError;

pub mod json_rpc;

#[derive(Debug)]
pub struct Backend {
    /// The backend that is used to communicate with the build server.
    inner: BackendImplementation,

    /// The API version that the backend supports.
    api_version: PixiBuildApiVersion,

    /// The backend capabilities that the backend support also taking into
    /// account the API version.
    capabilities: BackendCapabilities,
}

pub enum BackendImplementation {
    /// The backend is a JSON-RPC backend.
    JsonRpc(Box<json_rpc::JsonRpcBackend>),

    /// An in memory backend.
    InMemory(Box<dyn InMemoryBackend>),
}

impl Debug for BackendImplementation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.fmt(f),
            BackendImplementation::InMemory(backend) => f
                .debug_struct("InMemoryBackend")
                .field("identifier", &backend.identifier())
                .finish(),
        }
    }
}

impl BackendImplementation {
    pub fn capabilities(&self) -> BackendCapabilities {
        match self {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.capabilities().clone(),
            BackendImplementation::InMemory(in_memory) => in_memory.capabilities(),
        }
    }

    pub fn identifier(&self) -> &str {
        match self {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.identifier(),
            BackendImplementation::InMemory(in_memory) => in_memory.identifier(),
        }
    }

    pub fn version(&self) -> Option<&VersionWithSource> {
        match self {
            BackendImplementation::JsonRpc(json_rpc) => json_rpc.version(),
            BackendImplementation::InMemory(_) => None,
        }
    }
}

impl From<json_rpc::JsonRpcBackend> for BackendImplementation {
    fn from(json_rpc: json_rpc::JsonRpcBackend) -> Self {
        BackendImplementation::JsonRpc(Box::new(json_rpc))
    }
}

impl From<Box<dyn in_memory::InMemoryBackend>> for BackendImplementation {
    fn from(in_memory: Box<dyn in_memory::InMemoryBackend>) -> Self {
        BackendImplementation::InMemory(in_memory)
    }
}

impl Backend {
    pub fn new(inner: BackendImplementation, api_version: PixiBuildApiVersion) -> Self {
        let capabilities = inner.capabilities().mask_with_api_version(&api_version);
        Self {
            inner,
            api_version,
            capabilities,
        }
    }

    /// Returns an identifier for the backend. This is useful for debugging
    /// purposes mostly.
    pub fn identifier(&self) -> &str {
        self.inner.identifier()
    }

    /// Returns the version of the backend, if available. This is useful for
    /// debugging purposes mostly.
    pub fn version(&self) -> Option<&VersionWithSource> {
        self.inner.version()
    }

    /// Returns the capabilities of the backend. This takes into account both
    /// the actual capabilities of the backend and the API version that is in
    /// use.
    ///
    /// Sometimes backends provide more capabilities that the API version that
    /// we established. This can happen when the backend already implemented
    /// some capabilities both not all for a particular API version.
    pub fn capabilities(&self) -> &BackendCapabilities {
        &self.capabilities
    }

    /// Returns the capabilities of the backend, without taking into account the
    /// API version. This is only useful for debugging purposes. In most cases
    /// [`Self::capabilities`] should be used instead.
    pub fn backend_capabilities(&self) -> BackendCapabilities {
        self.inner.capabilities()
    }

    /// Returns the API version that was used to establish the backend.
    pub fn api_version(&self) -> PixiBuildApiVersion {
        self.api_version
    }

    pub async fn conda_build_v1<W: BackendOutputStream + Send + 'static>(
        &self,
        params: CondaBuildV1Params,
        output_stream: W,
    ) -> Result<CondaBuildV1Result, CommunicationError> {
        assert!(
            self.inner.capabilities().provides_conda_build_v1(),
            "This backend does not support the conda build v1 procedure"
        );
        match &self.inner {
            BackendImplementation::JsonRpc(json_rpc) => {
                json_rpc.conda_build_v1(params, output_stream).await
            }
            BackendImplementation::InMemory(in_memory) => in_memory
                .conda_build_v1(params, &output_stream)
                .map_err(|e| *e),
        }
    }

    /// Returns the outputs that this backend can produce.
    pub async fn conda_outputs<W: BackendOutputStream + Send + 'static>(
        &self,
        params: CondaOutputsParams,
        output_stream: W,
    ) -> Result<CondaOutputsResult, CommunicationError> {
        assert!(
            self.inner.capabilities().provides_conda_outputs(),
            "This backend does not support the conda outputs procedure"
        );
        match &self.inner {
            BackendImplementation::JsonRpc(json_rpc) => {
                json_rpc.conda_outputs(params, output_stream).await
            }
            BackendImplementation::InMemory(in_memory) => in_memory
                .conda_outputs(params, &output_stream)
                .map_err(|e| *e),
        }
    }
}

pub trait BackendOutputStream {
    /// An unstructured line produced by the backend, such as output from a
    /// compiler it spawned or anything it wrote to stderr directly.
    fn on_line(&mut self, line: String);

    /// A structured log event the backend sent over the connection.
    ///
    /// The default renders it as a line, so consumers that only care about
    /// human readable output do not have to implement anything. Override it to
    /// act on the level, the target or the fields.
    fn on_log(&mut self, log: LogParams) {
        self.on_line(render_log(&log));
    }
}

/// Render a structured log event for a consumer that does not care about the
/// structure.
///
/// The `BUILDER[...]` marker is what separates the backend's output from pixi's
/// own in a build log where the two interleave. Info is the ordinary case and
/// carries most of a build's output, so it is left unmarked -- a marker on every
/// line is a marker on none. What remains marked is what a reader is scanning
/// for: warnings, errors, and the levels they had to opt into seeing.
///
/// Only the marker is coloured: messages routinely arrive with their own escape
/// codes already in them (a compiler's diagnostics, for instance), and wrapping
/// those would fight with whatever colouring they came with.
fn render_log(log: &LogParams) -> String {
    let mut rendered = match level_marker(log.level) {
        Some((label, colour)) => render_prefix(label, colour),
        None => String::new(),
    };

    if let Some(target) = &log.target {
        rendered.push_str(&console::style(target).dim().to_string());
        rendered.push_str(": ");
    }
    rendered.push_str(&log.message);

    for (name, value) in &log.fields {
        rendered.push_str(&format!(" {name}={value}"));
    }

    rendered
}

/// Render a line the backend wrote straight to stderr.
///
/// These carry no level -- they are a subprocess' output, or a panic on the way
/// out -- so they get their own label rather than being dressed up as one.
fn render_stderr_line(line: &str) -> String {
    let mut rendered = render_prefix("STDERR", console::Color::Magenta);
    rendered.push_str(line);
    rendered
}

/// The `BUILDER[...]` marker, padded so messages line up across levels.
fn render_prefix(label: &str, colour: console::Color) -> String {
    format!(
        "{}{}{} ",
        console::style("BUILDER[").dim(),
        console::style(format!("{label:<6}")).fg(colour).bold(),
        console::style("]").dim(),
    )
}

/// How a level is marked, or `None` for the levels that go unmarked.
fn level_marker(level: LogLevel) -> Option<(&'static str, console::Color)> {
    match level {
        LogLevel::Trace => Some(("TRACE", console::Color::Cyan)),
        LogLevel::Debug => Some(("DEBUG", console::Color::Blue)),
        LogLevel::Info => None,
        LogLevel::Warn => Some(("WARN", console::Color::Yellow)),
        LogLevel::Error => Some(("ERROR", console::Color::Red)),
    }
}

impl BackendOutputStream for () {
    fn on_line(&mut self, _line: String) {
        // No-op implementation
    }
}

impl<F: FnMut(String)> BackendOutputStream for F {
    fn on_line(&mut self, line: String) {
        self(line);
    }
}

#[cfg(test)]
mod tests {
    use pixi_build_types::procedures::log::{LogLevel, LogParams};

    use super::{render_log, render_stderr_line};

    fn log(level: LogLevel, message: &str) -> LogParams {
        LogParams {
            level,
            message: message.to_string(),
            target: Some("pixi_build_cmake::build".to_string()),
            fields: [("package".to_string(), serde_json::json!("libfoo"))].into(),
        }
    }

    /// The marker is what lets a reader pick the backend's unusual lines out of
    /// a build log, so its shape is load bearing.
    #[test]
    fn marked_levels_are_labelled_and_aligned() {
        let levels = [
            (LogLevel::Trace, "TRACE"),
            (LogLevel::Debug, "DEBUG"),
            (LogLevel::Warn, "WARN"),
            (LogLevel::Error, "ERROR"),
        ];

        let mut widths = Vec::new();
        for (level, label) in levels {
            let rendered =
                console::strip_ansi_codes(&render_log(&log(level, "building"))).into_owned();
            assert!(
                rendered.starts_with(&format!("BUILDER[{label}")),
                "expected a {label} marker, got {rendered}"
            );
            assert!(rendered.contains("building"), "the message survives");
            assert!(
                rendered.contains("package=\"libfoo\""),
                "the fields survive: {rendered}"
            );
            widths.push(rendered.find(']').expect("the marker is closed"));
        }

        assert!(
            widths.windows(2).all(|pair| pair[0] == pair[1]),
            "labels must be padded to a common width so messages line up: {widths:?}"
        );
    }

    /// Info carries most of a build's output. Marking every one of those lines
    /// would drown the levels a reader is actually scanning for.
    #[test]
    fn info_is_not_marked() {
        let rendered =
            console::strip_ansi_codes(&render_log(&log(LogLevel::Info, "building"))).into_owned();

        assert!(
            !rendered.contains("BUILDER["),
            "info must carry no marker: {rendered}"
        );
        assert_eq!(
            rendered, "pixi_build_cmake::build: building package=\"libfoo\"",
            "everything except the marker is unchanged"
        );
    }

    /// Lines scraped off stderr have no level, so labelling them as one would
    /// be inventing information.
    #[test]
    fn stderr_lines_are_labelled_as_stderr() {
        let rendered =
            console::strip_ansi_codes(&render_stderr_line("ld: cannot find -lfoo")).into_owned();

        assert!(rendered.starts_with("BUILDER[STDERR"), "got {rendered}");
        assert!(
            rendered.ends_with("ld: cannot find -lfoo"),
            "got {rendered}"
        );
    }

    /// Backend messages often already contain escape codes of their own. Only
    /// the prefix may be coloured, or the two collide.
    #[test]
    fn only_the_prefix_is_coloured() {
        // Colours are off outside a terminal, which is the right default but
        // would make this assert nothing. The other tests in this module strip
        // escape codes, so flipping the global here does not disturb them.
        console::set_colors_enabled(true);

        let mut entry = log(LogLevel::Error, "\u{1b}[31malready red\u{1b}[0m");
        entry.target = None;
        entry.fields.clear();

        let rendered = render_log(&entry);
        let prefix = rendered
            .strip_suffix("\u{1b}[31malready red\u{1b}[0m")
            .expect("the message is passed through untouched, escape codes and all");

        assert!(
            prefix.contains('\u{1b}'),
            "the prefix is coloured: {prefix:?}"
        );
    }
}
