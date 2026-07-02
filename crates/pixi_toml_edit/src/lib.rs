//! Formatting-preserving edit helpers for [`toml_edit`] data structures.
//!
//! CLI commands rewrite manifests in place. The helpers in this crate mimic
//! the style that is already present in the document instead of imposing one:
//!
//! - overwriting an entry keeps its position and surrounding decor
//! - inserting into a multiline inline table or array puts the entry on its
//!   own line, copies the indentation of the existing entries, and always
//!   leaves a trailing comma
//! - inserting into a single-line inline table or array stays on one line
//! - removing an entry removes its whole line while the surviving entries
//!   keep their formatting
//!
//! Containers are never converted between single-line and multiline style.

mod array;
mod entry;
mod style;

pub use array::{insert_array_element, push_array_element, retain_array_elements};
pub use entry::{
    remove_entry, remove_inline_table_entry, upsert_entry, upsert_inline_table_entry,
    upsert_table_entry,
};

/// Error returned when an [`toml_edit::Item`] is expected to hold a table or
/// an inline table but holds something else.
#[derive(Debug, thiserror::Error)]
#[error("not a table-like value")]
pub struct NotATableError;
