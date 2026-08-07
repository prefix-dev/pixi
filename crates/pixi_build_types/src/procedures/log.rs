//! Log messages streamed from the backend to the frontend.
//!
//! Unlike the other procedures in this module this is a JSON-RPC
//! *notification*: the backend sends it spontaneously while a request is in
//! flight and the frontend never replies. Backends only emit it when the
//! frontend advertised `providesLogNotifications` in
//! [`crate::FrontendCapabilities`] during capability negotiation; otherwise
//! logging falls back to stderr.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const METHOD_NAME: &str = "log/message";

/// The severity of a log message.
///
/// This mirrors the levels of the `tracing` crate, which is what backends use
/// to emit them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// A single log message emitted by the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogParams {
    /// The severity of the message.
    pub level: LogLevel,

    /// The human readable message.
    pub message: String,

    /// The module path the message originated from, e.g.
    /// `pixi_build_cmake::build`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,

    /// Structured fields attached to the event, excluding the `message` field
    /// which is carried separately.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::{LogLevel, LogParams};

    /// The wire representation is part of the protocol: a backend and a
    /// frontend on different pixi versions must agree on it. Pins the casing of
    /// both the field names and the level values.
    #[test]
    fn log_params_wire_format_is_camel_case() {
        let params = LogParams {
            level: LogLevel::Warn,
            message: "recipe overrides the pinned version".to_string(),
            target: Some("pixi_build_cmake::build".to_string()),
            fields: [("package".to_string(), serde_json::json!("libfoo"))].into(),
        };

        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "level": "warn",
                "message": "recipe overrides the pinned version",
                "target": "pixi_build_cmake::build",
                "fields": { "package": "libfoo" },
            })
        );
    }

    /// `target` and `fields` are optional on the wire so the common case stays
    /// small. A payload without them must still deserialize.
    #[test]
    fn log_params_without_optional_fields_deserializes() {
        let params: LogParams =
            serde_json::from_value(serde_json::json!({ "level": "info", "message": "building" }))
                .unwrap();

        assert_eq!(params.level, LogLevel::Info);
        assert_eq!(params.target, None);
        assert!(params.fields.is_empty());

        // The optional members are skipped again on the way out.
        assert_eq!(
            serde_json::to_value(&params).unwrap(),
            serde_json::json!({ "level": "info", "message": "building" })
        );
    }
}
