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

    /// Retrieve a  reference to a target table `table_name`
    /// in dotted form (e.g. `table1.table2`) from the root of the document.
    pub fn get_nested_table<'a>(
        &'a self,
        table_name: &str,
    ) -> Result<&'a dyn TableLike, TomlError> {
        let parts: Vec<&str> = table_name.split('.').collect();

        let mut current_table = self.0.as_table() as &dyn TableLike;

        for part in parts {
            current_table = current_table
                .get(part)
                .ok_or_else(|| TomlError::table_error(part, table_name))?
                .as_table_like()
                .ok_or_else(|| TomlError::table_error(part, table_name))?;
        }
        Ok(current_table)
    }

    /// Retrieve a reference to a target table using pre-split parts.
    /// This allows for proper handling of escaped TOML keys.
    pub fn get_nested_table_from_parts<'a>(
        &'a self,
        parts: &[String],
    ) -> Result<&'a dyn TableLike, TomlError> {
        let table_name = parts.join(".");
        let mut current_table = self.0.as_table() as &dyn TableLike;

        for part in parts {
            current_table = current_table
                .get(part)
                .ok_or_else(|| TomlError::table_error(part, &table_name))?
                .as_table_like()
                .ok_or_else(|| TomlError::table_error(part, &table_name))?;
        }
        Ok(current_table)
    }

    /// Retrieve a mutable reference to a target table `table_name`
    /// in dotted form (e.g. `table1.table2`) from the root of the document.
    /// If the table is not found, it is inserted into the document.
    pub fn get_or_insert_nested_table<'a>(
        &'a mut self,
        table_name: &str,
    ) -> Result<&'a mut dyn TableLike, TomlError> {
        let parts: Vec<&str> = table_name.split('.').collect();
        self.get_or_insert_nested_table_from_parts(parts.iter().map(|s| s.to_string()).collect())
    }

    /// Retrieve a mutable reference to a target table using pre-split parts.
    /// This allows for proper handling of escaped TOML keys.
    pub fn get_or_insert_nested_table_from_parts(
        &mut self,
        parts: Vec<String>,
    ) -> Result<&mut dyn TableLike, TomlError> {
        let table_name = parts.join(".");
        let mut current_table = self.0.as_table_mut() as &mut dyn TableLike;

        for part in &parts {
            let entry = current_table.entry(part);
            let item = entry.or_insert(Item::Table(Table::new()));
            if let Some(table) = item.as_table_mut() {
                // Avoid creating empty tables
                table.set_implicit(true);
            }
            current_table = item
                .as_table_like_mut()
                .ok_or_else(|| TomlError::table_error(part, &table_name))?;
        }
        Ok(current_table)
    }

    /// Inserts a value into a certain table
    /// If the most inner table doesn't exist, an inline table will be created.
    /// If it already exists, the formatting of the table will be preserved
    pub fn insert_into_inline_table<'a>(
        &'a mut self,
        table_name: &str,
        key: &str,
        value: Value,
    ) -> Result<&'a mut dyn TableLike, TomlError> {
        let mut parts: Vec<&str> = table_name.split('.').collect();

        let last = parts.pop();

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

        // Add dependency as inline table if it doesn't exist
        if let Some(last) = last {
            if let Some(dependency) = current_table.get_mut(last) {
                dependency
                    .as_table_like_mut()
                    .map(|table| table.insert(key, Item::Value(value)));
            } else {
                let mut dependency = toml_edit::InlineTable::new();
                dependency.insert(key, value);
                current_table.insert(last, toml_edit::value(dependency));
            }
        }

        Ok(current_table)
    }

    /// Retrieves a mutable reference to a target array `array_name`
    /// in table `table_name` in dotted form (e.g. `table1.table2.array`).
    ///
    /// If the array is not found, it is inserted into the document.
    pub fn get_or_insert_toml_array_mut<'a>(
        &'a mut self,
        table_name: &str,
        array_name: &str,
    ) -> Result<&'a mut Array, TomlError> {
        self.get_or_insert_nested_table(table_name)?
            .entry(array_name)
            .or_insert(Item::Value(Value::Array(Array::new())))
            .as_array_mut()
            .ok_or_else(|| TomlError::array_error(array_name, table_name))
    }

    /// Retrieves a mutable reference to a target array `array_name`
    /// in table specified by pre-split parts.
    ///
    /// If the array is not found, it is inserted into the document.
    pub fn get_or_insert_toml_array_mut_from_parts<'a>(
        &'a mut self,
        table_parts: Vec<String>,
        array_name: &str,
    ) -> Result<&'a mut Array, TomlError> {
        let table_name = table_parts.join(".");
        self.get_or_insert_nested_table_from_parts(table_parts)?
            .entry(array_name)
            .or_insert(Item::Value(Value::Array(Array::new())))
            .as_array_mut()
            .ok_or_else(|| TomlError::array_error(array_name, &table_name))
    }

    /// Retrieves a mutable reference to a target array `array_name`
    /// in table `table_name` in dotted form (e.g. `table1.table2.array`).
    ///
    /// If the array is not found, returns None.
    pub fn get_mut_toml_array<'a>(
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

    /// Retrieves a reference to a target array `array_name`
    /// in table `table_name` in dotted form (e.g. `table1.table2.array`).
    ///
    /// If the array is not found, returns None.
    pub fn get_toml_array<'a>(
        &'a self,
        table_name: &str,
        array_name: &str,
    ) -> Result<Option<&'a Array>, TomlError> {
        let array = self
            .get_nested_table(table_name)?
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
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
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
        let mut manifest = TomlDocument::new(DocumentMut::from_str(toml).unwrap());
        manifest.get_or_insert_nested_table(table_name).unwrap();

        // No empty table is being created
        assert!(!manifest.0.to_string().contains("[test]"));
    }
}
