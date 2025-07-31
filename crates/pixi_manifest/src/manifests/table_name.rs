use std::fmt::{self, Display, Formatter};

use rattler_conda_types::Platform;

use crate::FeatureName;

/// Struct that is used to access a table in `pixi.toml` or `pyproject.toml`.
pub struct TableName<'a> {
    prefix: Option<&'static str>,
    platform: Option<&'a Platform>,
    feature_name: Option<&'a FeatureName>,
    table: Option<&'a str>,
}

impl Display for TableName<'_> {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.to_toml_table_name())
    }
}

impl<'a> TableName<'a> {
    /// Create a new `TableName` with default values.
    pub fn new() -> Self {
        Self {
            prefix: None,
            platform: None,
            feature_name: None,
            table: None,
        }
    }

    /// Set the prefix of the table.
    pub fn with_prefix(mut self, prefix: Option<&'static str>) -> Self {
        self.prefix = prefix;
        self
    }

    /// Set the platform of the table.
    pub fn with_platform(mut self, platform: Option<&'a Platform>) -> Self {
        self.platform = platform;
        self
    }

    /// Set the feature name of the table.
    pub fn with_feature_name(mut self, feature_name: Option<&'a FeatureName>) -> Self {
        self.feature_name = feature_name;
        self
    }

    /// Set the optional and custom table name.
    pub fn with_table(mut self, table: Option<&'static str>) -> Self {
        self.table = table;
        self
    }
}

impl TableName<'_> {
    /// Returns the name of the table in dotted form (e.g.
    /// `table1.table2.array`). It is composed of
    /// - the 'tool.pixi' prefix if the manifest is a 'pyproject.toml' file
    /// - the feature if it is not the default feature
    /// - the platform if it is not `None`
    /// - the name of a nested TOML table if it is not `None`
    fn to_toml_table_name(&self) -> String {
        let mut parts = Vec::new();

        if self.prefix.is_some() {
            parts.push(self.prefix.unwrap());
        }

        if let Some(feature_name) = self.feature_name.as_ref() {
            if !feature_name.is_default() {
                parts.push("feature");
                let feature_str = feature_name.as_str();
                parts.push(feature_str);
            }
        }

        if let Some(platform) = self.platform {
            parts.push("target");
            parts.push(platform.as_str());
        }

        if let Some(table) = self.table {
            parts.push(table);
        }

        parts.join(".")
    }

    /// Returns the individual parts of the table name for proper TOML key escaping.
    /// This allows the TOML library to handle escaping of individual keys correctly.
    pub fn to_parts(&self) -> Vec<String> {
        let mut parts = Vec::new();

        if let Some(prefix) = self.prefix {
            // Split the prefix by dots to handle cases like "tool.pixi"
            parts.extend(prefix.split('.').map(|s| s.to_string()));
        }

        if let Some(feature_name) = self.feature_name.as_ref() {
            if !feature_name.is_default() {
                parts.push("feature".to_string());
                let feature_str = feature_name.as_str();
                // Don't pre-escape - let the TOML library handle escaping
                parts.push(feature_str.to_string());
            }
        }

        if let Some(platform) = self.platform {
            parts.push("target".to_string());
            parts.push(platform.as_str().to_string());
        }

        if let Some(table) = self.table {
            parts.push(table.to_string());
        }

        parts
    }
}

#[cfg(test)]
mod tests {

    use insta::assert_snapshot;
    use pixi_spec::PixiSpec;
    use rattler_conda_types::{MatchSpec, ParseStrictness::Strict};
    use toml_edit::Item;

    use super::*;

    fn default_channel_config() -> rattler_conda_types::ChannelConfig {
        rattler_conda_types::ChannelConfig::default_with_root_dir(
            std::env::current_dir().expect("Could not retrieve the current directory"),
        )
    }

    #[test]
    fn test_nameless_to_toml() {
        let examples = [
            "rattler >=1",
            "conda-forge::rattler",
            "conda-forge::rattler[version=>3.0]",
            "rattler ==1 *cuda",
            "rattler >=1 *cuda",
        ];

        let channel_config = default_channel_config();
        let mut table = toml_edit::DocumentMut::new();
        for example in examples {
            let spec = MatchSpec::from_str(example, Strict)
                .unwrap()
                .into_nameless()
                .1;
            let spec = PixiSpec::from_nameless_matchspec(spec, &channel_config);
            table.insert(example, Item::Value(spec.to_toml_value()));
        }
        assert_snapshot!(table);
    }

    #[test]
    fn test_get_nested_toml_table_name() {
        // Test all different options for the feature name and platform
        assert_eq!(
            "dependencies".to_string(),
            TableName::new()
                .with_feature_name(Some(&FeatureName::DEFAULT))
                .with_table(Some("dependencies"))
                .to_string()
        );

        assert_eq!(
            "target.linux-64.dependencies".to_string(),
            TableName::new()
                .with_feature_name(Some(&FeatureName::DEFAULT))
                .with_platform(Some(&Platform::Linux64))
                .with_table(Some("dependencies"))
                .to_string()
        );

        let feature_name = FeatureName::from("test");
        assert_eq!(
            "feature.test.dependencies".to_string(),
            TableName::new()
                .with_feature_name(Some(&feature_name))
                .with_table(Some("dependencies"))
                .to_string()
        );

        assert_eq!(
            "feature.test.target.linux-64.dependencies".to_string(),
            TableName::new()
                .with_feature_name(Some(&feature_name))
                .with_platform(Some(&Platform::Linux64))
                .with_table(Some("dependencies"))
                .to_string()
        );

        // Test feature name with dot - no longer escaped in to_string()
        let feature_name = FeatureName::from("test.test");
        assert_eq!(
            "feature.test.test.dependencies".to_string(),
            TableName::new()
                .with_feature_name(Some(&feature_name))
                .with_table(Some("dependencies"))
                .to_string()
        );

        // But to_parts() should have the unescaped feature name (TOML library handles escaping)
        assert_eq!(
            vec!["feature", "test.test", "dependencies"],
            TableName::new()
                .with_feature_name(Some(&feature_name))
                .with_table(Some("dependencies"))
                .to_parts()
        );

        // Test with prefix - should split "tool.pixi" into separate parts
        assert_eq!(
            vec!["tool", "pixi", "dependencies"],
            TableName::new()
                .with_prefix(Some("tool.pixi"))
                .with_feature_name(Some(&FeatureName::DEFAULT))
                .with_table(Some("dependencies"))
                .to_parts()
        );

        // Test with prefix and feature - should split "tool.pixi" and keep feature name unescaped
        assert_eq!(
            vec!["tool", "pixi", "feature", "test.test", "dependencies"],
            TableName::new()
                .with_prefix(Some("tool.pixi"))
                .with_feature_name(Some(&feature_name))
                .with_table(Some("dependencies"))
                .to_parts()
        );
    }
}
