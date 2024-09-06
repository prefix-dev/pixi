use toml_edit::{self, Array, Item, Table, Value};

pub mod project;

pub mod manifest;

pub use project::ManifestSource;

use crate::error::TomlError;

#[derive(Debug, Clone, Default)]
pub struct TomlManifest(toml_edit::DocumentMut);

impl TomlManifest {
    pub fn new(document: toml_edit::DocumentMut) -> Self {
        Self(document)
    }

    /// Retrieve a mutable reference to a target table `table_name`
    /// in dotted form (e.g. `table1.table2`) from the root of the document.
    /// If the table is not found, it is inserted into the document.
    fn get_or_insert_nested_table<'a>(
        &'a mut self,
        table_name: &str,
    ) -> Result<&'a mut Table, TomlError> {
        // let parts = table_name.to_toml_table_name();
        // let table_name = table_name.to_string();

        let parts: Vec<&str> = table_name.split('.').collect();

        let mut current_table = self.0.as_table_mut();

        for part in parts {
            let entry = current_table.entry(part);
            let item = entry.or_insert(Item::Table(Table::new()));
            current_table = item
                .as_table_mut()
                .ok_or_else(|| TomlError::table_error(part, &table_name))?;
            // Avoid creating empty tables
            current_table.set_implicit(true);
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
