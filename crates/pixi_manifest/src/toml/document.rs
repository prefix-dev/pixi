use std::{
    fmt,
    fmt::{Display, Formatter},
};

use toml_edit::{Array, Decor, InlineTable, Item, Key, Table, TableLike, Value};

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
            if let Some(inline_table) = dependency.as_inline_table_mut() {
                insert_into_inline_table_with_format(inline_table, key, value);
            } else if let Some(table) = dependency.as_table_like_mut() {
                table.insert(key, Item::Value(value));
            }
        } else {
            let mut dependency = InlineTable::new();
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

/// Inserts a key-value pair into an inline table, respecting multiline
/// formatting. If the existing entries use newlines (i.e. the table is a TOML
/// 1.1 multiline inline table), the new entry is placed on its own line with
/// matching indentation. The trailing comma style of the table is preserved.
fn insert_into_inline_table_with_format(table: &mut InlineTable, key: &str, value: Value) {
    // Detect whether the table is multiline by checking if any existing key
    // has a newline in its prefix decoration.
    let indentation = table
        .iter()
        .filter_map(|(k, _)| {
            let prefix = table.key(k)?.leaf_decor().prefix()?.as_str()?;
            if prefix.contains('\n') {
                // Extract the indentation after the last newline
                let indent = prefix.rsplit_once('\n').map_or(prefix, |(_, after)| after);
                Some(indent.to_owned())
            } else {
                None
            }
        })
        .next();

    if let Some(indent) = indentation {
        let had_trailing_comma = table.trailing_comma();

        // Ensure the last existing value has a clean suffix so the comma
        // sits right after the value (no ` ,` artifacts).
        if let Some((last_key, _)) = table.iter().last() {
            let last_key = last_key.to_owned();
            if let Some(val) = table.get_mut(&last_key) {
                let suffix = val.decor().suffix().and_then(|s| s.as_str()).unwrap_or("");
                if suffix.trim().is_empty() {
                    val.decor_mut().set_suffix("");
                }
            }
        }

        let formatted_key = Key::new(key).with_leaf_decor(Decor::new(format!("\n{indent}"), " "));
        let mut formatted_value = value;
        // Clear any default suffix on the value so there is no space before
        // a potential trailing comma.
        formatted_value.decor_mut().set_suffix("");
        table.insert_formatted(&formatted_key, formatted_value);

        table.set_trailing_comma(had_trailing_comma);

        // Ensure the closing `}` stays on its own line by preserving the
        // trailing whitespace that precedes it.
        let trailing = table.trailing().as_str().unwrap_or("");
        if !trailing.contains('\n') {
            table.set_trailing("\n".to_string());
        }
    } else {
        table.insert(key, value);
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use toml_edit::{DocumentMut, InlineTable, Item};

    use super::insert_into_inline_table_with_format;
    use crate::toml::document::TomlDocument;

    /// Helper to parse a bare inline table string like `{ foo = "bar" }` into
    /// an [`InlineTable`], so we can test `insert_into_inline_table_with_format`
    /// directly without needing a full `[section]` header.
    fn parse_inline_table(s: &str) -> InlineTable {
        let doc = format!("t = {s}");
        let doc = DocumentMut::from_str(&doc).unwrap();
        doc.as_table()["t"].as_inline_table().unwrap().clone()
    }

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
        println!("Result: {result}");
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
        println!("get_or_insert_nested_table Result: {result}");
        // The key should be properly quoted in the section header
        assert!(result.contains("[envs.\"sdl.example\".dependencies]"));
    }

    #[test]
    fn test_feature_names_with_dots_issue_3171() {
        // Test case for issue #3171: Feature names with dots should be properly quoted
        let toml = r#"
[tool.pixi.workspace]
"#;
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        let result_table = manifest
            .get_or_insert_nested_table(&["tool", "pixi", "feature", "test.test", "dependencies"])
            .unwrap();

        // Add a dependency like "rich = '*'"
        result_table.insert("rich", toml_edit::Item::Value(toml_edit::Value::from("*")));

        let result = manifest.0.to_string();
        println!("Feature with dots result: {result}");

        // Should create [tool.pixi.feature."test.test".dependencies] not [tool.pixi.feature.test.test.dependencies]
        assert!(result.contains("[tool.pixi.feature.\"test.test\".dependencies]"));

        // Verify the dependency was added
        assert!(result.contains("rich = \"*\""));
    }

    #[test]
    fn test_feature_names_with_dots_inline_table_issue_3171() {
        // Test case for issue #3171 using insert_into_inline_table
        let toml = r#"
[tool.pixi.workspace]
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
        println!("Feature with dots inline table result: {result}");

        // Should create [tool.pixi.feature."test.test"] with dependencies = { rich = "*" }
        assert!(result.contains("[tool.pixi.feature.\"test.test\"]"));

        // Verify the dependency was added as inline table
        assert!(result.contains("dependencies = { rich = \"*\" }"));
    }

    /// Tests that multiline inline tables (TOML 1.1) can be parsed by
    /// toml_edit and round-trip correctly.
    #[test]
    fn test_multiline_inline_table_parsing() {
        let toml = r#"
[envs.python]
channels = ["dummy-channel"]
dependencies = {
    dummy = "3.11.*",
    other = "1.0",
}
"#;
        let manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        insta::assert_snapshot!(manifest.0.to_string(), @r#"

        [envs.python]
        channels = ["dummy-channel"]
        dependencies = {
            dummy = "3.11.*",
            other = "1.0",
        }
        "#);
    }

    // ---- Direct tests for insert_into_inline_table_with_format ----

    /// Single-entry multiline WITH trailing comma (inverse of the Hofer-Julian
    /// bug case).
    #[test]
    fn test_format_single_entry_with_trailing_comma() {
        let mut table = parse_inline_table("{\n    numpy = \"*\",\n}");
        insert_into_inline_table_with_format(&mut table, "scipy", toml_edit::Value::from(">=1.0"));
        insta::assert_snapshot!(table.to_string(), @r#"
         {
            numpy = "*",
            scipy = ">=1.0",
        }
        "#);
    }

    /// Sequential insertions — insert two entries one after another into the
    /// same table (the real-world `pixi add foo && pixi add bar` scenario).
    #[test]
    fn test_format_sequential_insertions() {
        let mut table = parse_inline_table("{\n    numpy = \"*\",\n}");
        insert_into_inline_table_with_format(&mut table, "scipy", toml_edit::Value::from(">=1.0"));
        insert_into_inline_table_with_format(&mut table, "pandas", toml_edit::Value::from(">=2.0"));
        insta::assert_snapshot!(table.to_string(), @r#"
         {
            numpy = "*",
            scipy = ">=1.0",
            pandas = ">=2.0",
        }
        "#);
    }

    /// Empty multiline table — verifies fallback to plain insert.
    #[test]
    fn test_format_empty_table() {
        let mut table = parse_inline_table("{}");
        insert_into_inline_table_with_format(&mut table, "foo", toml_edit::Value::from("1.0"));
        insta::assert_snapshot!(table.to_string(), @r#" { foo = "1.0" }"#);
    }

    /// Tab indentation — verifies tabs are preserved, not converted to spaces.
    #[test]
    fn test_format_tab_indentation() {
        let mut table = parse_inline_table("{\n\tnumpy = \"*\",\n}");
        insert_into_inline_table_with_format(&mut table, "scipy", toml_edit::Value::from(">=1.0"));
        insta::assert_snapshot!(table.to_string(), @r#"
         {
        	numpy = "*",
        	scipy = ">=1.0",
        }
        "#);
    }

    /// Single-line table through the format function — verifies the else branch
    /// (non-multiline) preserves single-line formatting.
    #[test]
    fn test_format_single_line_table() {
        let mut table = parse_inline_table("{ numpy = \"*\" }");
        insert_into_inline_table_with_format(&mut table, "scipy", toml_edit::Value::from(">=1.0"));
        insta::assert_snapshot!(table.to_string(), @r#" { numpy = "*" , scipy = ">=1.0" }"#);
    }

    /// Empty multiline table with newline (`{\n}`) — a valid TOML 1.1 construct
    /// with no entries but containing whitespace. No key has a newline prefix,
    /// so it should fall back to plain insert.
    #[test]
    fn test_format_empty_multiline_table() {
        let mut table = parse_inline_table("{\n}");
        insert_into_inline_table_with_format(&mut table, "foo", toml_edit::Value::from("1.0"));
        // Normalize trailing whitespace (toml_edit Display artifact) for snapshot
        let result = table
            .to_string()
            .lines()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        insta::assert_snapshot!(result, @r#"
         { foo = "1.0"
        }
        "#);
    }

    /// Updating an existing key in a multiline inline table — verifies that
    /// re-inserting a key that already exists replaces the value without
    /// corrupting formatting.
    #[test]
    fn test_format_update_existing_key() {
        let mut table = parse_inline_table("{\n    numpy = \"1.0\",\n    scipy = \"2.0\",\n}");
        insert_into_inline_table_with_format(&mut table, "numpy", toml_edit::Value::from(">=2.0"));
        insta::assert_snapshot!(table.to_string(), @r#"
         {
            numpy = ">=2.0",
            scipy = "2.0",
        }
        "#);
    }

    /// Non-dependency inline table — verifies the function works on any inline
    /// table, not just dependencies (e.g. a build backend spec).
    #[test]
    fn test_format_non_dependency_inline_table() {
        let mut table =
            parse_inline_table("{\n    name = \"pixi-build-python\",\n    version = \"*\",\n}");
        insert_into_inline_table_with_format(
            &mut table,
            "channel",
            toml_edit::Value::from("conda-forge"),
        );
        insta::assert_snapshot!(table.to_string(), @r#"
         {
            name = "pixi-build-python",
            version = "*",
            channel = "conda-forge",
        }
        "#);
    }

    /// Tests that inserting into a multiline inline table preserves the
    /// multiline format.
    #[test]
    fn test_insert_into_multiline_inline_table() {
        let toml = r#"
[envs.python]
channels = ["dummy-channel"]
dependencies = {
    dummy = "3.11.*",
    other = "1.0",
}
"#;
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        manifest
            .insert_into_inline_table(
                &["envs", "python", "dependencies"],
                "new-pkg",
                toml_edit::Value::from("2.0"),
            )
            .unwrap();

        insta::assert_snapshot!(manifest.0.to_string(), @r#"

        [envs.python]
        channels = ["dummy-channel"]
        dependencies = {
            dummy = "3.11.*",
            other = "1.0",
            new-pkg = "2.0",
        }
        "#);
    }

    /// Tests inserting into a multiline inline table where the last element
    /// does NOT have a trailing comma. The new entry should not add one either.
    #[test]
    fn test_insert_into_multiline_inline_table_no_trailing_comma() {
        let toml = r#"
[envs.python]
channels = ["dummy-channel"]
dependencies = {
    dummy = "3.11.*",
    other = "1.0"
}
"#;
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        manifest
            .insert_into_inline_table(
                &["envs", "python", "dependencies"],
                "new-pkg",
                toml_edit::Value::from("2.0"),
            )
            .unwrap();

        insta::assert_snapshot!(manifest.0.to_string(), @r#"

        [envs.python]
        channels = ["dummy-channel"]
        dependencies = {
            dummy = "3.11.*",
            other = "1.0",
            new-pkg = "2.0"
        }
        "#);
    }

    /// Reproduces the bug from https://github.com/prefix-dev/pixi/pull/5655#issuecomment-4040362659
    /// where inserting into a single-entry multiline inline table without a
    /// trailing comma produces `{ numpy = "*"\n, pydantic = ... }`.
    #[test]
    fn test_insert_into_multiline_inline_table_single_entry() {
        let toml = r#"
[feature.test]
dependencies = {
    numpy = "*"
}
"#;
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        manifest
            .insert_into_inline_table(
                &["feature", "test", "dependencies"],
                "pydantic",
                toml_edit::Value::from(">=2.12.5,<3"),
            )
            .unwrap();

        insta::assert_snapshot!(manifest.0.to_string(), @r#"

        [feature.test]
        dependencies = {
            numpy = "*",
            pydantic = ">=2.12.5,<3"
        }
        "#);
    }
}
