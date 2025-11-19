use serde::Deserialize;
use serde_json::Value as JsonValue;
use tracing::Level;

const BACKEND_PREFIX: &str = "BACKEND";
const TARGET_ATTRIBUTE: &str = "target";

#[derive(Debug, Clone)]
pub struct BackendLogEntry {
    pub level: Level,
    lines: Vec<String>,
}

impl BackendLogEntry {
    fn new(level: Level, lines: Vec<String>) -> Self {
        Self { level, lines }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }
}

pub fn parse_backend_logs(line: &str) -> Option<Vec<BackendLogEntry>> {
    parse_otlp_logs(line).or_else(|| parse_legacy_backend_log(line).map(|entry| vec![entry]))
}

fn parse_otlp_logs(line: &str) -> Option<Vec<BackendLogEntry>> {
    let data: TracesData = serde_json::from_str(line)
        .map_err(|err| {
            tracing::debug!(target: "pixi::backend_logs", "Failed to parse OTLP line: {err}");
            err
        })
        .ok()?;

    let mut entries = Vec::new();
    for span in data.spans() {
        let mut span_emitted = false;
        for event in &span.events {
            if let Some(entry) = entry_from_event(span, event) {
                span_emitted = true;
                entries.push(entry);
            }
        }

        if !span_emitted {
            if let Some(entry) = entry_from_span(span) {
                entries.push(entry);
            }
        }
    }

    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

fn entry_from_event(span: &OtlpSpan, event: &OtlpEvent) -> Option<BackendLogEntry> {
    let mut level = Level::INFO;
    let mut target = None;
    let mut message = None;

    for attribute in &event.attributes {
        match attribute.key.as_str() {
            "level" => {
                if let Some(parsed) = attribute
                    .value
                    .as_string()
                    .and_then(|value| parse_level(&value))
                {
                    level = parsed;
                }
            }
            TARGET_ATTRIBUTE => target = attribute.value.as_string(),
            "message" => message = attribute.value.as_string(),
            _ => {}
        }
    }

    let mut extras =
        collect_extra_attributes(&event.attributes, &["level", TARGET_ATTRIBUTE, "message"]);
    if extras.is_empty() {
        extras = collect_extra_attributes(&span.attributes, &[TARGET_ATTRIBUTE]);
    }

    let descriptor = span_descriptor(span);
    let rendered_message = build_message(event, message, extras);
    let lines = backend_lines(level, descriptor, target, rendered_message);

    Some(BackendLogEntry::new(level, lines))
}

fn entry_from_span(span: &OtlpSpan) -> Option<BackendLogEntry> {
    let descriptor = span_descriptor(span)?;
    let extra_fields = collect_extra_attributes(&span.attributes, &[TARGET_ATTRIBUTE]);

    let message = if extra_fields.is_empty() {
        String::new()
    } else {
        extra_fields
            .into_iter()
            .map(|(key, value)| format!("{key}={}", maybe_quote(&value)))
            .collect::<Vec<_>>()
            .join(" ")
    };

    let lines = backend_lines(Level::INFO, Some(descriptor), None, message);
    Some(BackendLogEntry::new(Level::INFO, lines))
}

fn build_message(
    event: &OtlpEvent,
    provided_message: Option<String>,
    extras: Vec<(String, String)>,
) -> String {
    let mut message = provided_message
        .filter(|msg| !msg.is_empty())
        .or_else(|| (!event.name.is_empty()).then(|| event.name.clone()))
        .unwrap_or_default();

    if !extras.is_empty() {
        if !message.is_empty() {
            message.push(' ');
        }
        message.push_str(
            &extras
                .into_iter()
                .map(|(key, value)| format!("{key}={}", maybe_quote(&value)))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }

    message
}

fn backend_lines(
    level: Level,
    descriptor: Option<String>,
    target: Option<String>,
    message: String,
) -> Vec<String> {
    let mut context = String::new();
    if let Some(descriptor) = descriptor {
        context.push_str(&descriptor);
    }
    if let Some(target) = target {
        if !context.is_empty() {
            context.push_str(": ");
        }
        context.push_str(&target);
    }

    let mut lines = Vec::new();
    let mut message_lines = message.lines();
    let mut first_line = String::new();
    if !context.is_empty() {
        first_line.push_str(&context);
    }
    if let Some(first_message_line) = message_lines.next() {
        if !first_message_line.is_empty() {
            if !first_line.is_empty() {
                first_line.push_str(": ");
            }
            first_line.push_str(first_message_line);
        }
    }
    let mut head = format!("{BACKEND_PREFIX} {}", level.as_str());
    if !first_line.is_empty() {
        head.push(' ');
        head.push_str(&first_line);
    }
    lines.push(head);

    for line in message_lines {
        let mut continuation = format!("{BACKEND_PREFIX} {}", level.as_str());
        if !line.is_empty() {
            continuation.push(' ');
            continuation.push_str(line);
        }
        lines.push(continuation);
    }

    lines
}

fn span_descriptor(span: &OtlpSpan) -> Option<String> {
    if span.name.is_empty() {
        return None;
    }

    let fields = collect_extra_attributes(&span.attributes, &[TARGET_ATTRIBUTE]);
    if fields.is_empty() {
        Some(span.name.clone())
    } else {
        Some(format!(
            "{}{{{}}}",
            span.name,
            fields
                .into_iter()
                .map(|(key, value)| format!("{key}={}", maybe_quote(&value)))
                .collect::<Vec<_>>()
                .join(" ")
        ))
    }
}

fn collect_extra_attributes(attributes: &[KeyValue], ignore: &[&str]) -> Vec<(String, String)> {
    attributes
        .iter()
        .filter(|attr| !ignore.contains(&attr.key.as_str()))
        .map(|attr| (attr.key.clone(), attr.value.to_display_string()))
        .collect()
}

fn maybe_quote(value: &str) -> String {
    if value.chars().any(|c| c.is_whitespace()) {
        format!("\"{}\"", value.replace('\"', "\\\""))
    } else {
        value.to_string()
    }
}

fn parse_legacy_backend_log(line: &str) -> Option<BackendLogEntry> {
    let value: JsonValue = serde_json::from_str(line).ok()?;
    let level = match value.get("level")?.as_str()? {
        "TRACE" => Level::TRACE,
        "DEBUG" => Level::DEBUG,
        "INFO" => Level::INFO,
        "WARN" => Level::WARN,
        "ERROR" => Level::ERROR,
        _ => return None,
    };

    let target = value
        .get("target")
        .and_then(JsonValue::as_str)
        .map(|s| s.to_owned());

    let message = value
        .get("fields")
        .and_then(|fields| {
            if let Some(msg) = fields.get("message").and_then(JsonValue::as_str) {
                (!msg.is_empty()).then(|| msg.to_owned())
            } else if let Some(obj) = fields.as_object() {
                (!obj.is_empty()).then(|| fields.to_string())
            } else if let Some(text) = fields.as_str() {
                (!text.is_empty()).then(|| text.to_owned())
            } else {
                None
            }
        })
        .unwrap_or_else(|| line.to_owned());

    let lines = backend_lines(level, None, target, message);
    Some(BackendLogEntry::new(level, lines))
}

#[derive(Debug, Deserialize)]
struct TracesData {
    #[serde(rename = "resourceSpans", default)]
    resource_spans: Vec<ResourceSpans>,
}

impl TracesData {
    fn spans(&self) -> impl Iterator<Item = &OtlpSpan> {
        self.resource_spans
            .iter()
            .flat_map(|resource| resource.scope_spans.iter())
            .flat_map(|scope| scope.spans.iter())
    }
}

#[derive(Debug, Deserialize)]
struct ResourceSpans {
    #[serde(rename = "scopeSpans", default)]
    scope_spans: Vec<ScopeSpans>,
}

#[derive(Debug, Deserialize)]
struct ScopeSpans {
    #[serde(rename = "spans", default)]
    spans: Vec<OtlpSpan>,
}

#[derive(Debug, Deserialize)]
struct OtlpSpan {
    #[serde(default)]
    name: String,
    #[serde(default)]
    attributes: Vec<KeyValue>,
    #[serde(default)]
    events: Vec<OtlpEvent>,
}

#[derive(Debug, Deserialize)]
struct OtlpEvent {
    #[serde(default)]
    name: String,
    #[serde(default)]
    attributes: Vec<KeyValue>,
}

#[derive(Debug, Deserialize)]
struct KeyValue {
    key: String,
    value: AnyValue,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::enum_variant_names)]
enum AnyValue {
    StringValue(String),
    BoolValue(bool),
    IntValue(String),
    DoubleValue(f64),
    ArrayValue {
        #[serde(default)]
        values: Vec<AnyValue>,
    },
    KvlistValue {
        #[serde(default)]
        values: Vec<KeyValue>,
    },
}

impl AnyValue {
    fn as_string(&self) -> Option<String> {
        match self {
            AnyValue::StringValue(value) => Some(value.clone()),
            AnyValue::BoolValue(value) => Some(value.to_string()),
            AnyValue::IntValue(value) => Some(value.clone()),
            AnyValue::DoubleValue(value) => Some(value.to_string()),
            _ => None,
        }
    }

    fn to_display_string(&self) -> String {
        match self {
            AnyValue::StringValue(value) => value.clone(),
            AnyValue::BoolValue(value) => value.to_string(),
            AnyValue::IntValue(value) => value.clone(),
            AnyValue::DoubleValue(value) => value.to_string(),
            AnyValue::ArrayValue { values } => {
                let parts: Vec<String> = values.iter().map(AnyValue::to_display_string).collect();
                format!("[{}]", parts.join(", "))
            }
            AnyValue::KvlistValue { values } => {
                let parts: Vec<String> = values
                    .iter()
                    .map(|kv| format!("{}={}", kv.key, kv.value.to_display_string()))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
        }
    }
}

fn parse_level(value: &str) -> Option<Level> {
    match value.to_ascii_uppercase().as_str() {
        "TRACE" => Some(Level::TRACE),
        "DEBUG" => Some(Level::DEBUG),
        "INFO" => Some(Level::INFO),
        "WARN" | "WARNING" => Some(Level::WARN),
        "ERROR" => Some(Level::ERROR),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sample_otlp_line() {
        let sample = include_str!("../../tests/otlp_sample.json");
        let entries = parse_backend_logs(sample).expect("failed to parse sample");
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0]
                .lines()
                .iter()
                .any(|line| line.starts_with("BACKEND INFO Fetching source code"))
        );
    }

    #[test]
    fn parses_runtime_sample() {
        let line = r#"{"resourceSpans":[{"resource":{"attributes":[{"key":"telemetry.sdk.language","value":{"stringValue":"rust"}},{"key":"service.name","value":{"stringValue":"pixi-build-backend"}},{"key":"telemetry.sdk.version","value":{"stringValue":"0.31.0"}},{"key":"telemetry.sdk.name","value":{"stringValue":"opentelemetry"}}]},"scopeSpans":[{"scope":{"name":"pixi-build-backend"},"spans":[{"traceId":"e66e8f27ecea6455191e84fe19e163c8","spanId":"c33bcbb9fd422b85","parentSpanId":"0859b080e6e4a8b9","flags":257,"name":"Resolving environments","kind":1,"startTimeUnixNano":"1763548765738750120","endTimeUnixNano":"1763548765738751242","attributes":[{"key":"code.file.path","value":{"stringValue":"/home/remi/.cargo/git/checkouts/rattler-build-5c6d2407a545b614/d4d9a38/src/render/resolved_dependencies.rs"}},{"key":"code.module.name","value":{"stringValue":"rattler_build::render::resolved_dependencies"}},{"key":"code.line.number","value":{"intValue":"1057"}},{"key":"thread.id","value":{"intValue":"1"}},{"key":"thread.name","value":{"stringValue":"main"}},{"key":"target","value":{"stringValue":"rattler_build::render::resolved_dependencies"}},{"key":"busy_ns","value":{"intValue":"310"}},{"key":"idle_ns","value":{"intValue":"912"}}]}]}]}]}"#;
        assert!(parse_backend_logs(line).is_some());
    }

    #[test]
    fn parses_legacy_tracing_json() {
        let line = r#"{"timestamp":"2025-03-03T13:37:00.000000Z","level":"WARN","target":"pixi_build_cmake","fields":{"message":"cmake warning: foo"}}"#;
        let entries = parse_backend_logs(line).expect("should parse legacy log");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, Level::WARN);
        assert!(
            entries[0]
                .lines()
                .iter()
                .any(|line| line.contains("cmake warning: foo"))
        );
    }
}
