use indexmap::IndexSet;
use miette::{IntoDiagnostic, Report};
use rattler_conda_types::{Channel, PackageName, Platform};
use rattler_repodata_gateway::Gateway;
use regex::Regex;
use std::sync::LazyLock;
use strsim::jaro;

/// Extracts package name from "No candidates were found" error messages
pub fn extract_failed_package_name(error: &Report) -> Option<String> {
    static PACKAGE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"No candidates were found for ([a-zA-Z0-9_-]+)(?:\s+\*)?")
            .expect("Invalid regex")
    });

    let error_chain = std::iter::successors(Some(error.as_ref() as &dyn std::error::Error), |e| {
        e.source()
    });

    for error in error_chain {
        if let Some(captures) = PACKAGE_REGEX.captures(&error.to_string()) {
            return captures.get(1).map(|m| m.as_str().to_string());
        }
    }
    None
}

pub struct PackageSuggester {
    channels: IndexSet<Channel>,
    platform: Platform,
    gateway: Gateway,
}

impl PackageSuggester {
    pub fn new(channels: IndexSet<Channel>, platform: Platform, gateway: Gateway) -> Self {
        Self {
            channels,
            platform,
            gateway,
        }
    }

    /// Get all package names using CEP-0016 shard index
    async fn get_all_package_names(&self) -> miette::Result<Vec<PackageName>> {
        self.gateway
            .names(self.channels.clone(), [self.platform, Platform::NoArch])
            .await
            .into_diagnostic()
    }

    /// Get suggestions using fast shard index lookup
    pub async fn suggest_similar(&self, failed_package: &str) -> miette::Result<Vec<String>> {
        let all_names = self.get_all_package_names().await?;

        // Simple but fast approach: collect matches and similarities in one pass
        let failed_lower = failed_package.to_lowercase();
        let mut matches: Vec<(f64, String)> = Vec::new();

        // Single pass through packages with early termination for good matches
        for pkg in &all_names {
            let name = pkg.as_normalized();
            let name_lower = name.to_lowercase();

            // Skip exact matches to avoid suggesting the same package
            if name_lower == failed_lower {
                continue;
            }

            // Quick wins first (fast string operations)
            let score =
                if name_lower.starts_with(&failed_lower) || failed_lower.starts_with(&name_lower) {
                    0.9 // Prefix match (high priority)
                } else if name_lower.contains(&failed_lower) {
                    0.8 // Substring match (medium priority) 
                } else {
                    // Only compute expensive Jaro for potential matches
                    let jaro_score = jaro(&name_lower, &failed_lower);
                    if jaro_score > 0.6 { jaro_score } else { 0.0 }
                };

            if score > 0.0 {
                matches.push((score, name.to_string()));

                // Early termination if we have enough good matches
                if matches.len() >= 10 && score > 0.8 {
                    break;
                }
            }
        }

        // Sort by score (highest first) and take top 3
        matches.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let suggestions: Vec<String> = matches.into_iter().take(3).map(|(_, name)| name).collect();

        Ok(suggestions)
    }
}

pub fn create_enhanced_package_error(
    failed_package: &str,
    suggestions: &[String],
) -> miette::Report {
    let mut help_text = String::new();

    if !suggestions.is_empty() {
        help_text.push_str("Did you mean one of these?\n");
        for suggestion in suggestions {
            help_text.push_str(&format!("  - {}\n", suggestion));
        }
        help_text.push('\n');
    }

    help_text.push_str(&format!(
        "tip: a similar subcommand exists: 'search {}'",
        failed_package
    ));

    miette::miette!(
        help = help_text,
        "No candidates were found for '{}'",
        failed_package
    )
}

//Todo: Add tests maybe
