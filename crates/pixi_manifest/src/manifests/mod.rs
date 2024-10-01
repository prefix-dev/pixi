use toml_edit::{self, Array, Item, Table, TableLike, Value};

pub mod project;

pub mod manifest;

pub use project::ManifestSource;

use crate::error::TomlError;

/// Represents a wrapper around a TOML document.
/// This struct is exposed to other crates
/// to allow for easy manipulation of the TOML document.
#[derive(Debug, Clone, Default)]
pub struct TomlManifest(toml_edit::DocumentMut);

impl TomlManifest {
    /// Create a new `TomlManifest` from a `toml_edit::DocumentMut` document.
    pub fn new(document: toml_edit::DocumentMut) -> Self {
        Self(document)
    }

    /// Retrieve a mutable reference to a target table `table_name`
    /// in dotted form (e.g. `table1.table2`) from the root of the document.
    /// If the table is not found, it is inserted into the document.
    fn get_or_insert_nested_table<'a>(
        &'a mut self,
        table_name: &str,
    ) -> Result<&'a mut dyn TableLike, TomlError> {
        let parts: Vec<&str> = table_name.split('.').collect();

        let mut current_table = self.0.as_table_mut() as &mut dyn TableLike;

        for part in parts {
            let entry = current_table.entry(part);
            let item = entry.or_insert(Item::Table(Table::new()));
            if let Some(table) = item.as_table_mut() {
                // Avoid creating empty tables
                table.set_implicit(true);
            }
            current_table = item
                .as_table_like_mut()
                .ok_or_else(|| TomlError::table_error(part, table_name))?;
        }
        Ok(current_table)
    }

    /// Retrieves a mutable reference to a target array `array_name`
    /// in table `table_name` in dotted form (e.g. `table1.table2.array`).
    ///
    /// If the array is not found, it is inserted into the document.
    pub fn get_or_insert_toml_array<'a>(
        &'a mut self,
        table_name: &str,
        array_name: &str,
    ) -> Result<&'a mut Array, TomlError> {
        self.get_or_insert_nested_table(table_name)?
            .entry(array_name)
            .or_insert(Item::Value(Value::Array(Array::new())))
            .as_array_mut()
            .ok_or_else(|| TomlError::array_error(array_name, table_name.to_string().as_str()))
    }

    /// Retrieves a mutable reference to a target array `array_name`
    /// in table `table_name` in dotted form (e.g. `table1.table2.array`).
    ///
    /// If the array is not found, returns None.
    pub fn get_toml_array<'a>(
        &'a mut self,
        table_name: &str,
        array_name: &str,
    ) -> Result<Option<&'a mut Array>, TomlError> {
        let array = self
            .get_or_insert_nested_table(table_name)?
            .get_mut(array_name)
            .and_then(|a| a.as_array_mut());
        Ok(array)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use toml_edit::DocumentMut;

    use super::*;

    #[test]
    fn test_get_or_insert_nested_table() {
        let toml = r#"
[envs.python]
channels = ["dummy-channel"]
[envs.python.dependencies]
dummy = "3.11.*"
"#;
        let dep_name = "test";
        let mut manifest = TomlManifest::new(DocumentMut::from_str(toml).unwrap());
        manifest
            .get_or_insert_nested_table("envs.python.dependencies")
            .unwrap()
            .insert(dep_name, Item::Value(toml_edit::Value::from("6.6")));

        let dep = manifest
            .get_or_insert_nested_table("envs.python.dependencies")
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
        let mut manifest = TomlManifest::new(DocumentMut::from_str(toml).unwrap());
        manifest
            .get_or_insert_nested_table("envs.python.dependencies")
            .unwrap()
            .insert(dep_name, Item::Value(toml_edit::Value::from("6.6")));

        let dep = manifest
            .get_or_insert_nested_table("envs.python.dependencies")
            .unwrap()
            .get(dep_name);

        assert!(dep.is_some());

        // Existing entries are also still there
        let dummy = manifest
            .get_or_insert_nested_table("envs.python.dependencies")
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
        let mut manifest = TomlManifest::new(DocumentMut::from_str(toml).unwrap());
        manifest.get_or_insert_nested_table(table_name).unwrap();

        // No empty table is being created
        assert!(!manifest.0.to_string().contains("[test]"));
    }
}
