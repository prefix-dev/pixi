//! ROS distribution index client.
//!
//! Fetches the ROS distribution index from GitHub and extracts distribution
//! metadata (ROS1 vs ROS2, Python version, package list).

use std::{collections::HashMap, path::Path};

use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use miette::Diagnostic;
use reqwest_middleware::ClientBuilder;
use reqwest_retry::RetryTransientMiddleware;
use retry_policies::policies::ExponentialBackoff;
use serde::Deserialize;
use thiserror::Error;

const INDEX_URL: &str = "https://raw.githubusercontent.com/ros/rosdistro/master/index-v4.yaml";

/// Number of times a transient failure (e.g. HTTP 429) is retried before giving
/// up on the ROS distribution index request.
const MAX_RETRIES: u32 = 3;

/// Errors that can occur when fetching ROS distribution info.
#[derive(Debug, Error, Diagnostic)]
pub enum DistroError {
    #[error("failed to fetch ROS distribution index")]
    Fetch(#[from] reqwest_middleware::Error),

    #[error(
        "the ROS distribution index at {url} is being rate limited (HTTP 429 Too Many Requests)"
    )]
    #[diagnostic(help(
        "GitHub throttled the request. The response is cached on disk once it \
         succeeds, so retrying shortly usually resolves this. If it persists, \
         wait a few minutes before building again."
    ))]
    RateLimited { url: String },

    #[error("the ROS distribution index request to {url} failed with HTTP {status}")]
    Status { url: String, status: u16 },

    #[error("failed to read ROS distribution index response body")]
    Body(#[from] reqwest::Error),

    #[error("failed to parse ROS distribution index YAML")]
    ParseIndex(#[source] serde_yaml::Error),

    #[error("distribution '{name}' not found in ROS distribution index")]
    #[diagnostic(help("Available distributions can be found at https://github.com/ros/rosdistro"))]
    NotFound { name: String },
}

/// Information about a ROS distribution.
#[derive(Debug, Clone)]
pub struct Distro {
    pub name: String,
    pub is_ros1: bool,
    pub python_version: Option<String>,
}

impl Distro {
    /// Fetch distribution info from the ROS distribution index.
    ///
    /// When `http_cache_dir` is provided, the index response is cached on disk so
    /// repeated backend invocations within the same workspace avoid hitting the
    /// network. GitHub rate limits the unauthenticated `raw.githubusercontent.com`
    /// request (HTTP 429), so it is also retried with exponential backoff.
    pub async fn fetch(name: &str, http_cache_dir: Option<&Path>) -> Result<Self, DistroError> {
        let client = reqwest::Client::new();
        let mut builder = ClientBuilder::from_client(client.into());

        // Cache outermost: a fresh cached index skips the network entirely.
        if let Some(cache_dir) = http_cache_dir {
            builder = builder.with(Cache(HttpCache {
                mode: CacheMode::Default,
                manager: CACacheManager {
                    path: cache_dir.to_path_buf(),
                    remove_opts: Default::default(),
                },
                options: HttpCacheOptions::default(),
            }));
        }

        // Retry innermost: retries transient 429/5xx, honoring `Retry-After`.
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(MAX_RETRIES);
        builder = builder.with(RetryTransientMiddleware::new_with_policy(retry_policy));

        let client = builder.build();

        let response = client.get(INDEX_URL).send().await?;

        // Retries exhausted still return the last (error) response; check the
        // status so a 429 body isn't misreported as a YAML parse failure.
        let status = response.status();
        if !status.is_success() {
            return Err(if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                DistroError::RateLimited {
                    url: INDEX_URL.to_string(),
                }
            } else {
                DistroError::Status {
                    url: INDEX_URL.to_string(),
                    status: status.as_u16(),
                }
            });
        }

        let index_yaml = response.text().await?;

        let index: DistroIndex =
            serde_yaml::from_str(&index_yaml).map_err(DistroError::ParseIndex)?;

        let entry = index.distributions.get(name).ok_or(DistroError::NotFound {
            name: name.to_string(),
        })?;

        let is_ros1 = entry
            .distribution_type
            .as_deref()
            .map(|t| t == "ros1")
            .unwrap_or(false);

        Ok(Distro {
            name: name.to_string(),
            is_ros1,
            python_version: entry.python_version.clone(),
        })
    }

    /// Create a builder for constructing a Distro without fetching from the network.
    #[cfg(test)]
    pub fn builder(name: &str) -> DistroBuilder {
        DistroBuilder {
            name: name.to_string(),
            is_ros1: false,
            python_version: None,
        }
    }

    /// Returns the mutex package name for this distro.
    pub fn ros_distro_mutex_name(&self) -> String {
        if self.is_ros1 {
            "ros-distro-mutex".to_string()
        } else {
            "ros2-distro-mutex".to_string()
        }
    }
}

/// Builder for constructing a [`Distro`] in tests.
#[cfg(test)]
pub struct DistroBuilder {
    name: String,
    is_ros1: bool,
    python_version: Option<String>,
}

#[cfg(test)]
impl DistroBuilder {
    pub fn ros1(mut self, is_ros1: bool) -> Self {
        self.is_ros1 = is_ros1;
        self
    }

    pub fn python_version(mut self, version: impl Into<String>) -> Self {
        self.python_version = Some(version.into());
        self
    }

    pub fn build(self) -> Distro {
        Distro {
            name: self.name,
            is_ros1: self.is_ros1,
            python_version: self.python_version,
        }
    }
}

/// The top-level ROS distribution index (index-v4.yaml).
#[derive(Debug, Deserialize)]
struct DistroIndex {
    distributions: HashMap<String, DistroEntry>,
}

/// An entry in the distribution index.
#[derive(Debug, Deserialize)]
struct DistroEntry {
    distribution_type: Option<String>,
    python_version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distro_ros1() {
        let distro = Distro::builder("noetic").ros1(true).build();
        assert_eq!(distro.name, "noetic");
        assert!(distro.is_ros1);
        assert_eq!(distro.ros_distro_mutex_name(), "ros-distro-mutex");
    }

    #[test]
    fn test_distro_ros_2() {
        let distro = Distro::builder("jazzy").build();
        assert_eq!(distro.name, "jazzy");
        assert!(!distro.is_ros1);
        assert_eq!(distro.ros_distro_mutex_name(), "ros2-distro-mutex");
    }

    #[test]
    fn test_parse_index_yaml() {
        let yaml = r#"
distributions:
  noetic:
    distribution:
      - https://example.com/noetic/distribution.yaml
    distribution_type: ros1
    python_version: "3"
  jazzy:
    distribution:
      - https://example.com/jazzy/distribution.yaml
    distribution_type: ros2
    python_version: "3"
"#;

        let index: DistroIndex = serde_yaml::from_str(yaml).unwrap();
        assert!(index.distributions.contains_key("noetic"));
        assert!(index.distributions.contains_key("jazzy"));
        assert_eq!(
            index.distributions["noetic"].distribution_type.as_deref(),
            Some("ros1")
        );
        assert_eq!(
            index.distributions["jazzy"].distribution_type.as_deref(),
            Some("ros2")
        );
    }
}
