use std::{
    fmt,
    fmt::{Display, Formatter},
};

use toml_edit::{Array, Item, Table, TableLike, Value};

use crate::TomlError;

/// Represents a wrapper around a TOML document.
///
/// This struct is exposed to other crates to allow for easy manipulation of the
/// TOML document.
#[derive(Debug, Clone, Default)]
pub struct TomlDocument(toml_edit::DocumentMut);

impl Display for TomlDocument {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TomlDocument {
    /// Create a new `TomlManifest` from a `toml_edit::DocumentMut` document.
    pub fn new(document: toml_edit::DocumentMut) -> Self {
        Self(document)
    }

    /// Returns the manifest as a mutable table
    pub fn as_table_mut(&mut self) -> &mut Table {
        self.0.as_table_mut()
    }

    /// Returns the manifest as a mutable table
    pub fn as_table(&self) -> &Table {
        self.0.as_table()
    }

    /// Get or insert a top-level item
    pub fn get_or_insert<'a>(&'a mut self, key: &str, item: Item) -> &'a Item {
        self.0.entry(key).or_insert(item)
    }

    /// Retrieve a reference to a target table using key array.
    pub fn get_nested_table<'a>(&'a self, keys: &[&str]) -> Result<&'a dyn TableLike, TomlError> {
        let mut current_table = self.0.as_table() as &dyn TableLike;

        for part in keys {
            current_table = current_table
                .get(part)
                .ok_or_else(|| TomlError::table_error(part, &keys.join(".")))?
                .as_table_like()
                .ok_or_else(|| TomlError::table_error(part, &keys.join(".")))?;
        }
        Ok(current_table)
    }

    /// Retrieve a mutable reference to a target table using key array.
    /// If the table is not found, it is inserted into the document.
    pub fn get_or_insert_nested_table<'a>(
        &'a mut self,
        keys: &[&str],
    ) -> Result<&'a mut dyn TableLike, TomlError> {
        let mut current_table = self.0.as_table_mut() as &mut dyn TableLike;

        for part in keys {
            let entry = current_table.entry(part);
            let item = entry.or_insert(Item::Table(Table::new()));
            if let Some(table) = item.as_table_mut() {
                // Avoid creating empty tables
                table.set_implicit(true);
            }
            current_table = item
                .as_table_like_mut()
                .ok_or_else(|| TomlError::table_error(part, &keys.join(".")))?;
        }
        Ok(current_table)
    }

    pub fn get_or_insert_toml_array_mut<'a>(
        &'a mut self,
        keys: &[&str],
        array_name: &str,
    ) -> Result<&'a mut Array, TomlError> {
        self.get_or_insert_nested_table(keys)?
            .entry(array_name)
            .or_insert(Item::Value(Value::Array(Array::new())))
            .as_array_mut()
            .ok_or_else(|| TomlError::array_error(array_name, &keys.join(".")))
    }

    pub fn insert_into_inline_table<'a>(
        &'a mut self,
        keys: &[&str],
        key: &str,
        value: Value,
    ) -> Result<&'a mut dyn TableLike, TomlError> {
        if keys.is_empty() {
            return Err(TomlError::table_error("", "empty keys array"));
        }

        // Split the keys into all but the last, and the last key
        let (parent_keys, last_key) = keys.split_at(keys.len() - 1);

        let mut current_table = self.0.as_table_mut() as &mut dyn TableLike;

        // Navigate to the parent table
        for part in parent_keys {
            let entry = current_table.entry(part);
            let item = entry.or_insert(Item::Table(Table::new()));
            if let Some(table) = item.as_table_mut() {
                // Avoid creating empty tables
                table.set_implicit(true);
            }
            current_table = item
                .as_table_like_mut()
                .ok_or_else(|| TomlError::table_error(part, &keys.join(".")))?;
        }

        let last_key = last_key[0];

        // Add dependency as inline table if it doesn't exist
        if let Some(dependency) = current_table.get_mut(last_key) {
            dependency
                .as_table_like_mut()
                .map(|table| table.insert(key, Item::Value(value)));
        } else {
            let mut dependency = toml_edit::InlineTable::new();
            dependency.insert(key, value);
            current_table.insert(last_key, toml_edit::value(dependency));
        }

        Ok(current_table)
    }

    pub fn get_mut_toml_array<'a>(
        &'a mut self,
        keys: &[&str],
        array_name: &str,
    ) -> Result<Option<&'a mut Array>, TomlError> {
        let table = self.get_or_insert_nested_table(keys)?;
        Ok(table
            .get_mut(array_name)
            .and_then(|item| item.as_array_mut()))
    }

    /// Retrieves a reference to a target array `array_name`
    /// using key array for table access.
    ///
    /// If the array is not found, returns None.
    pub fn get_toml_array<'a>(
        &'a self,
        keys: &[&str],
        array_name: &str,
    ) -> Result<Option<&'a Array>, TomlError> {
        let array = self
            .get_nested_table(keys)?
            .get(array_name)
            .and_then(|a| a.as_array());
        Ok(array)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use toml_edit::{DocumentMut, Item};

    use crate::toml::document::TomlDocument;

    #[test]
    fn test_get_or_insert_nested_table() {
        let toml = r#"
[envs.python]
channels = ["dummy-channel"]
[envs.python.dependencies]
dummy = "3.11.*"
"#;
        let dep_name = "test";
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        manifest
            .get_or_insert_nested_table(&["envs", "python", "dependencies"])
            .unwrap()
            .insert(dep_name, Item::Value(toml_edit::Value::from("6.6")));

        let dep = manifest
            .get_or_insert_nested_table(&["envs", "python", "dependencies"])
            .unwrap()
            .get(dep_name);

        assert!(dep.is_some());
    }

    #[test]
    fn test_get_or_insert_inline_table() {
        let toml = r#"
[envs.python]
channels = ["dummy-channel"]
dependencies = { dummy = "3.11.*" }
"#;
        let dep_name = "test";
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        manifest
            .get_or_insert_nested_table(&["envs", "python", "dependencies"])
            .unwrap()
            .insert(dep_name, Item::Value(toml_edit::Value::from("6.6")));

        let dep = manifest
            .get_or_insert_nested_table(&["envs", "python", "dependencies"])
            .unwrap()
            .get(dep_name);

        assert!(dep.is_some());

        // Existing entries are also still there
        let dummy = manifest
            .get_or_insert_nested_table(&["envs", "python", "dependencies"])
            .unwrap()
            .get("dummy");

        assert!(dummy.is_some())
    }

    #[test]
    fn test_get_or_insert_nested_table_no_empty_tables() {
        let toml = r#"
[envs.python]
channels = ["dummy-channel"]
"#;
        let table_name = "test";
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        manifest.get_or_insert_nested_table(&[table_name]).unwrap();

        // No empty table is being created
        assert!(!manifest.0.to_string().contains("[test]"));
    }

    #[test]
    fn test_insert_into_inline_table_preserves_inline_format() {
        let toml = r#"
[envs.python]
channels = ["dummy-channel"]
dependencies = { dummy = "3.11.*" }
"#;
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        manifest
            .insert_into_inline_table(
                &["envs", "python", "dependencies"],
                "test",
                toml_edit::Value::from("6.6"),
            )
            .unwrap();

        // Should preserve inline table format
        let result = manifest.0.to_string();
        assert!(result.contains("dependencies = {"));
        assert!(result.contains("test = \"6.6\""));
    }

    #[test]
    fn test_insert_into_inline_table_with_quoted_keys() {
        let toml = r#"
[envs]
"#;
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        manifest
            .insert_into_inline_table(
                &["envs", "sdl.example", "dependencies"],
                "test",
                toml_edit::Value::from("1.0"),
            )
            .unwrap();

        // Should create proper quoted section names
        let result = manifest.0.to_string();
        println!("Result: {}", result);
        assert!(result.contains("[envs.\"sdl.example\"]"));
    }

    #[test]
    fn test_get_or_insert_nested_table_with_quoted_keys() {
        let toml = r#"
[envs]
"#;
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        let result_table = manifest
            .get_or_insert_nested_table(&["envs", "sdl.example", "dependencies"])
            .unwrap();

        // Add something to the table to make it non-empty
        result_table.insert(
            "test",
            toml_edit::Item::Value(toml_edit::Value::from("1.0")),
        );

        // Should create proper quoted section names
        let result = manifest.0.to_string();
        println!("get_or_insert_nested_table Result: {}", result);
        // The key should be properly quoted in the section header
        assert!(result.contains("[envs.\"sdl.example\".dependencies]"));
    }

    #[test]
    fn test_feature_names_with_dots_issue_3171() {
        // Test case for issue #3171: Feature names with dots should be properly quoted
        let toml = r#"
[tool.pixi.project]
"#;
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        let result_table = manifest
            .get_or_insert_nested_table(&["tool", "pixi", "feature", "test.test", "dependencies"])
            .unwrap();

        // Add a dependency like "rich = '*'"
        result_table.insert("rich", toml_edit::Item::Value(toml_edit::Value::from("*")));

        let result = manifest.0.to_string();
        println!("Feature with dots result: {}", result);

        // Should create [tool.pixi.feature."test.test".dependencies] not [tool.pixi.feature.test.test.dependencies]
        assert!(result.contains("[tool.pixi.feature.\"test.test\".dependencies]"));

        // Verify the dependency was added
        assert!(result.contains("rich = \"*\""));
    }

    #[test]
    fn test_feature_names_with_dots_inline_table_issue_3171() {
        // Test case for issue #3171 using insert_into_inline_table
        let toml = r#"
[tool.pixi.project]
"#;
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        manifest
            .insert_into_inline_table(
                &["tool", "pixi", "feature", "test.test", "dependencies"],
                "rich",
                toml_edit::Value::from("*"),
            )
            .unwrap();

        let result = manifest.0.to_string();
        println!("Feature with dots inline table result: {}", result);

        // Should create [tool.pixi.feature."test.test"] with dependencies = { rich = "*" }
        assert!(result.contains("[tool.pixi.feature.\"test.test\"]"));

        // Verify the dependency was added as inline table
        assert!(result.contains("dependencies = { rich = \"*\" }"));
    }
}
