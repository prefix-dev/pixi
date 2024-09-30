use std::{fmt, thread::current};

use toml_edit::{self, value, Array, InlineTable, Item, Table, TableLike, Value};

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
    pub fn get_or_insert_nested_table<'a>(
        &'a mut self,
        table_name: &str,
    ) -> Result<&'a mut Table, TomlError> {
        let parts: Vec<&str> = table_name.split('.').collect();

        let mut current_table = self.0.as_table_mut();

        for part in parts {
            let entry = current_table.entry(part);
            let item = entry.or_insert(Item::Table(Table::new()));
            current_table = item
                .as_table_mut()
                .ok_or_else(|| TomlError::table_error(part, table_name))?;
            // Avoid creating empty tables
            current_table.set_implicit(true);
        }
        Ok(current_table)
    }

    pub fn get_or_insert_inline_table<'a>(
        &'a mut self,
        table_name: &str,
    ) -> Result<&'a mut InlineTable, TomlError> {
        let parts: Vec<&str> = table_name.split('.').collect();

        let mut current_table = self.0.as_table_mut();

        for (index, part) in parts.iter().enumerate() {
            if current_table.contains_table(part) {
                current_table = current_table
                    .get_mut(part)
                    .unwrap()
                    .as_table_mut()
                    .ok_or_else(|| TomlError::table_error(part, table_name))?;
                // Avoid creating empty tables
                current_table.set_implicit(true);
                continue;
            }

            if index + 1 == parts.len() {
                let new_table = current_table
                    .entry(part)
                    .or_insert(Item::Table(Table::new()))
                    .as_table()
                    .ok_or_else(|| TomlError::table_error(part, table_name))?;
                current_table[part] = value(new_table.clone().into_inline_table());
                return Ok(current_table
                    .get_mut(part)
                    .unwrap()
                    .as_inline_table_mut()
                    .unwrap());
            }

            let new_table = current_table
                .entry(part)
                .or_insert(Item::Table(Table::new()))
                .as_table_mut()
                .ok_or_else(|| TomlError::table_error(part, table_name))?;
            // Avoid creating empty tables
            new_table.set_implicit(true);
            current_table = new_table;
        }
        unreachable!();
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

impl fmt::Display for TomlManifest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
