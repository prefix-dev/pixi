use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use thiserror::Error;
use url::Url;

/// Custom S3 configuration
#[serde_as]
#[derive(Debug, Clone, PartialEq, Serialize, Eq, Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct S3Options {
    /// S3 endpoint URL
    pub endpoint_url: Option<Url>,
    /// Name of the region
    pub region: Option<String>,
    /// Force path style URLs instead of subdomain style
    pub force_path_style: Option<bool>,
}

impl S3Options {
    pub fn union(&self, other: &S3Options) -> Result<S3Options, S3OptionsMergeError> {
        let endpoint_url = match (self.endpoint_url.clone(), other.endpoint_url.clone()) {
            (Some(first), Some(second)) => {
                if first == second {
                    Some(first)
                } else {
                    return Err(S3OptionsMergeError::MultipleEndpoints{ first, second });
                }
            }
            (Some(endpoint), None) => Some(endpoint),
            (None, Some(endpoint)) => Some(endpoint),
            (None, None) => None,
        };

        let region = match (self.region.clone(), other.region.clone()) {
            (Some(first), Some(second)) => {
                if first == second {
                    Some(first)
                } else {
                    return Err(S3OptionsMergeError::MultipleRegions { first, second });
                }
            }
            (Some(region), None) => Some(region),
            (None, Some(region)) => Some(region),
            (None, None) => None,
        };

        let force_path_style = match (self.force_path_style, other.force_path_style) {
            (Some(first), Some(second)) => {
                if first == second {
                    Some(first)
                } else {
                    return Err(S3OptionsMergeError::MultipleForcePathStyles{ first, second });
                }
            }
            (Some(force_path_style), None) => Some(force_path_style),
            (None, Some(force_path_style)) => Some(force_path_style),
            (None, None) => None,
        };

        Ok(S3Options {
            endpoint_url,
            region,
            force_path_style,
        })
    }
}

#[derive(Error, Debug)]
pub enum S3OptionsMergeError {
    #[error(
        "multiple primary pypi indexes are not supported, found both {first} and {second} across multiple pypi options"
    )]
    MultipleEndpoints { first: Url, second: Url },
    #[error(
        "multiple regions are not allowed, found both {first} and {second} across multiple S3 options"
    )]
    MultipleRegions { first: String, second: String },
    #[error(
        "multiple force-path-styles are not allowed, found both {first} and {second} across multiple S3 options"
    )]
    MultipleForcePathStyles { first: bool, second: bool },
}
