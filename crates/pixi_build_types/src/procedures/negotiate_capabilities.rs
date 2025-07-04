//! This API was introduced in Pixi Build API version 0.

use serde::{Deserialize, Serialize};

use crate::capabilities::{BackendCapabilities, FrontendCapabilities};

pub const METHOD_NAME: &str = "negotiateCapabilities";

/// Negotiate the capabilities between the frontend and the backend.
/// after which we know what the backend can do and what the frontend can do.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NegotiateCapabilitiesParams {
    /// The capabilities that the frontend provides.
    pub capabilities: FrontendCapabilities,
}

/// The result of the initialize request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NegotiateCapabilitiesResult {
    /// The capabilities that the backend provides.
    pub capabilities: BackendCapabilities,
}
