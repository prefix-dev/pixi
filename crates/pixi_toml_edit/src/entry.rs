use toml_edit::{Decor, InlineTable, Item, Key, Table, Value};

use crate::{
    NotATableError,
    style::{
        decor_prefix, decor_suffix, detach_suffix, detect_inline_table_style,
        drop_first_line_comment, indent_after_newline, merge_comments, raw_contains_newline,
        raw_str, split_first_line_comment, standalone_comment_lines,
    },
};

/// Inserts `value` under `key` in a table or inline table, or overwrites the
/// existing entry while keeping its position and decor.
///
/// New entries in a multiline inline table go on their own line, copying the
/// indentation of the existing entries and keeping a trailing comma behind
/// the last entry. Single-line inline tables and regular tables get the
/// default `toml_edit` formatting.
pub fn upsert_entry(container: &mut Item, key: &str, value: Value) -> Result<(), NotATableError> {
    match container {
        Item::Table(table) => {
            upsert_table_entry(table, key, value);
            Ok(())
        }
        Item::Value(Value::InlineTable(table)) => {
            upsert_inline_table_entry(table, key, value);
            Ok(())
        }
        _ => Err(NotATableError),
    }
}

/// Removes the entry under `key` from a table or inline table, including the
/// line it occupies in a multiline inline table.
pub fn remove_entry(container: &mut Item, key: &str) -> Result<Option<Item>, NotATableError> {
    match container {
        Item::Table(table) => Ok(table.remove(key)),
        Item::Value(Value::InlineTable(table)) => {
            Ok(remove_inline_table_entry(table, key).map(Item::Value))
        }
        _ => Err(NotATableError),
    }
}

/// See [`upsert_entry`].
pub fn upsert_table_entry(table: &mut Table, key: &str, value: Value) {
    if let Some(existing) = table.get_mut(key).and_then(Item::as_value_mut) {
        overwrite_value(existing, value);
        return;
    }
    table.insert(key, Item::Value(value));
}

/// See [`upsert_entry`].
pub fn upsert_inline_table_entry(table: &mut InlineTable, key: &str, value: Value) {
    if let Some(existing) = table.get_mut(key) {
        overwrite_value(existing, value);
        return;
    }

    let style = detect_inline_table_style(table);
    if !style.is_multiline() {
        // Detach any stray whitespace behind the previously last value so the
        // separator comma ends up directly behind it.
        detach_last_value_suffix(table);
        table.insert(key, value);
        return;
    }

    // A comment on the previously last entry's line is stored behind that
    // entry, either in its value's suffix or (behind the trailing comma) in
    // the table's trailing decor. Detach it and re-attach it in front of the
    // new entry's line break so it stays on the line it was written on.
    let mut comment = detach_last_value_suffix(table);
    if let Some((trailing_comment, rest)) = split_first_line_comment(raw_str(table.trailing())) {
        comment = merge_comments(comment, Some(trailing_comment));
        table.set_trailing(rest);
    }

    let prefix = style.new_entry_prefix(comment.as_deref());
    let key = Key::new(key).with_leaf_decor(Decor::new(prefix, " "));
    table.insert_formatted(&key, value.decorated(" ", ""));
    table.set_trailing_comma(true);
    if !raw_contains_newline(table.trailing()) {
        table.set_trailing("\n");
    }
}

/// See [`remove_entry`].
pub fn remove_inline_table_entry(table: &mut InlineTable, key: &str) -> Option<Value> {
    let was_multiline = detect_inline_table_style(table).is_multiline();

    let keys: Vec<String> = table.iter().map(|(key, _)| key.to_string()).collect();
    let position = keys.iter().position(|existing| existing == key)?;

    // A comment on the removed entry's line is stored in the decor of
    // whatever follows it: the next key's prefix, or the table's trailing
    // decor if the removed entry was the last one. Drop it so it dies with
    // the line it was written on. Standalone comment lines in front of the
    // removed entry keep their own lines, so they move along instead.
    let removed_prefix = table
        .key(key)
        .map(|removed_key| decor_prefix(removed_key.leaf_decor()).to_string())
        .unwrap_or_default();
    let standalone = standalone_comment_lines(&removed_prefix).map(str::to_string);
    let indent = indent_after_newline(&removed_prefix).unwrap_or_default();
    if let Some(next_key) = keys.get(position + 1) {
        if let Some(mut next_key) = table.key_mut(next_key) {
            let prefix = decor_prefix(next_key.leaf_decor()).to_string();
            let mut new_prefix = drop_first_line_comment(&prefix);
            if let Some(standalone) = &standalone {
                // Whatever follows the standalone lines must start on a fresh
                // line, or it would be swallowed by the comment.
                if !new_prefix.starts_with('\n') {
                    new_prefix = format!("\n{indent}");
                }
                new_prefix = format!("{standalone}{new_prefix}");
            }
            next_key.leaf_decor_mut().set_prefix(new_prefix);
        }
    } else {
        let trailing = raw_str(table.trailing()).to_string();
        let mut new_trailing = drop_first_line_comment(&trailing);
        if let Some(standalone) = &standalone {
            // The closing brace must start on a fresh line, or it would be
            // swallowed by the comment.
            if !new_trailing.starts_with('\n') {
                new_trailing = String::from("\n");
            }
            new_trailing = format!("{standalone}{new_trailing}");
        }
        table.set_trailing(new_trailing);
    }

    let removed_last = position == keys.len() - 1;
    let removed = table.remove(key)?;

    if table.is_empty() {
        if raw_str(table.trailing()).trim().is_empty() {
            table.set_trailing("");
        }
        table.set_trailing_comma(false);
    } else if was_multiline
        && removed_last
        && !raw_contains_newline(table.trailing())
        && !last_value_suffix_has_newline(table)
    {
        // The removed entry carried the line break in front of the closing
        // brace; put it back so the brace stays on its own line. Append
        // rather than overwrite: the trailing decor may hold a comment that
        // was detached from a surviving entry's line.
        let trailing = raw_str(table.trailing()).to_string();
        table.set_trailing(format!("{trailing}\n"));
    }
    Some(removed)
}

/// Replaces `existing` with `value` while keeping the decor, so comments and
/// spacing around the value survive the overwrite.
fn overwrite_value(existing: &mut Value, mut value: Value) {
    *value.decor_mut() = existing.decor().clone();
    *existing = value;
}

fn detach_last_value_suffix(table: &mut InlineTable) -> Option<String> {
    let last_key = table.iter().last().map(|(key, _)| key.to_string())?;
    let value = table.get_mut(&last_key)?;
    detach_suffix(value.decor_mut())
}

fn last_value_suffix_has_newline(table: &InlineTable) -> bool {
    table
        .iter()
        .last()
        .is_some_and(|(_, value)| decor_suffix(value.decor()).contains('\n'))
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use toml_edit::{DocumentMut, Value};

    use super::{remove_entry, upsert_entry};

    fn upsert_in(toml: &str, path: &[&str], key: &str, value: &str) -> String {
        let mut doc: DocumentMut = toml.parse().unwrap();
        let mut item = doc.as_item_mut();
        for part in path {
            item = &mut item[part];
        }
        let value: Value = value.parse().unwrap();
        upsert_entry(item, key, value).unwrap();
        doc.to_string()
    }

    fn remove_in(toml: &str, path: &[&str], key: &str) -> String {
        let mut doc: DocumentMut = toml.parse().unwrap();
        let mut item = doc.as_item_mut();
        for part in path {
            item = &mut item[part];
        }
        remove_entry(item, key).unwrap();
        doc.to_string()
    }

    const DEPS: &[&str] = &["dependencies"];

    #[test]
    fn insert_into_single_line_inline_table() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = { numpy = "*" }
        "#,
                DEPS,
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"dependencies = { numpy = "*", pydantic = ">=2,<3" }"#
        );
    }

    #[test]
    fn insert_into_multiline_inline_table_with_trailing_comma() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = {
    numpy = "*",
}
"#,
                DEPS,
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"
        dependencies = {
            numpy = "*",
            pydantic = ">=2,<3",
        }
        "#
        );
    }

    #[test]
    fn insert_into_multiline_inline_table_without_trailing_comma() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = {
    numpy = "*"
}
"#,
                DEPS,
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"
        dependencies = {
            numpy = "*",
            pydantic = ">=2,<3",
        }
        "#
        );
    }

    #[test]
    fn insert_keeps_comment_on_last_entry_line() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = {
    numpy = "*", # the classic
}
"#,
                DEPS,
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"
        dependencies = {
            numpy = "*", # the classic
            pydantic = ">=2,<3",
        }
        "#
        );
    }

    #[test]
    fn insert_keeps_comment_on_last_entry_line_without_trailing_comma() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = {
    numpy = "*" # the classic
}
"#,
                DEPS,
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"
        dependencies = {
            numpy = "*", # the classic
            pydantic = ">=2,<3",
        }
        "#
        );
    }

    #[test]
    fn insert_keeps_comments_between_entries() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = {
    # scientific stack
    numpy = "*",
    scipy = "*",
}
"#,
                DEPS,
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"
        dependencies = {
            # scientific stack
            numpy = "*",
            scipy = "*",
            pydantic = ">=2,<3",
        }
        "#
        );
    }

    #[test]
    fn insert_into_empty_inline_table() {
        assert_snapshot!(
            upsert_in(
                "dependencies = {}\n",
                DEPS,
                "numpy",
                r#""*""#
            ),
            @r#"dependencies = { numpy = "*" }"#
        );
    }

    #[test]
    fn insert_into_empty_multiline_inline_table() {
        assert_snapshot!(
            upsert_in(
                "dependencies = {\n}\n",
                DEPS,
                "numpy",
                r#""*""#
            ),
            @r#"
        dependencies = {
            numpy = "*",
        }
        "#
        );
    }

    #[test]
    fn insert_into_regular_table() {
        assert_snapshot!(
            upsert_in(
                r#"[dependencies]
numpy = "*"
"#,
                DEPS,
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"
        [dependencies]
        numpy = "*"
        pydantic = ">=2,<3"
        "#
        );
    }

    #[test]
    fn insert_mimics_two_space_indent() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = {
  numpy = "*",
}
"#,
                DEPS,
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"
        dependencies = {
          numpy = "*",
          pydantic = ">=2,<3",
        }
        "#
        );
    }

    #[test]
    fn insert_mimics_tab_indent() {
        assert_snapshot!(
            upsert_in(
                "dependencies = {\n\tnumpy = \"*\",\n}\n",
                DEPS,
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"
        dependencies = {
        	numpy = "*",
        	pydantic = ">=2,<3",
        }
        "#
        );
    }

    #[test]
    fn insert_keeps_closing_brace_indentation() {
        assert_snapshot!(
            upsert_in(
                r#"[feature.test]
dependencies = {
        numpy = "*",
    }
"#,
                &["feature", "test", "dependencies"],
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"
        [feature.test]
        dependencies = {
                numpy = "*",
                pydantic = ">=2,<3",
            }
        "#
        );
    }

    #[test]
    fn insert_with_first_entry_on_brace_line() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = { numpy = "*",
  scipy = "*" }
"#,
                DEPS,
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"
        dependencies = { numpy = "*",
          scipy = "*",
          pydantic = ">=2,<3",
        }
        "#
        );
    }

    #[test]
    fn insert_inline_table_value_into_multiline_inline_table() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = {
    numpy = "*",
}
"#,
                DEPS,
                "pytorch-cpu",
                r#"{ version = "~=1.1", channel = "pytorch" }"#
            ),
            @r#"
        dependencies = {
            numpy = "*",
            pytorch-cpu = { version = "~=1.1", channel = "pytorch" },
        }
        "#
        );
    }

    #[test]
    fn overwrite_in_single_line_inline_table_keeps_position() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = { numpy = "*", scipy = "*" }
        "#,
                DEPS,
                "numpy",
                r#"">=2""#
            ),
            @r#"dependencies = { numpy = ">=2", scipy = "*" }"#
        );
    }

    #[test]
    fn overwrite_in_multiline_inline_table_keeps_line() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = {
    numpy = "*", # pinned soon
    scipy = "*",
}
"#,
                DEPS,
                "numpy",
                r#"">=2""#
            ),
            @r#"
        dependencies = {
            numpy = ">=2", # pinned soon
            scipy = "*",
        }
        "#
        );
    }

    #[test]
    fn overwrite_in_regular_table_keeps_comment() {
        assert_snapshot!(
            upsert_in(
                r#"[dependencies]
numpy = "*" # the classic
scipy = "*"
"#,
                DEPS,
                "numpy",
                r#"">=2""#
            ),
            @r#"
        [dependencies]
        numpy = ">=2" # the classic
        scipy = "*"
        "#
        );
    }

    #[test]
    fn remove_middle_entry_from_multiline_inline_table() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = {
    numpy = "*",
    scipy = "*",
    pandas = "*",
}
"#,
                DEPS,
                "scipy"
            ),
            @r#"
        dependencies = {
            numpy = "*",
            pandas = "*",
        }
        "#
        );
    }

    #[test]
    fn remove_last_entry_from_multiline_inline_table() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = {
    numpy = "*",
    scipy = "*",
}
"#,
                DEPS,
                "scipy"
            ),
            @r#"
        dependencies = {
            numpy = "*",
        }
        "#
        );
    }

    #[test]
    fn remove_last_entry_without_trailing_comma() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = {
    numpy = "*",
    scipy = "*"
}
"#,
                DEPS,
                "scipy"
            ),
            @r#"
        dependencies = {
            numpy = "*"
        }
        "#
        );
    }

    #[test]
    fn remove_entry_with_comment_drops_the_comment() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = {
    numpy = "*", # the classic
    scipy = "*",
}
"#,
                DEPS,
                "numpy"
            ),
            @r#"
        dependencies = {
            scipy = "*",
        }
        "#
        );
    }

    #[test]
    fn remove_last_entry_with_comment_drops_the_comment() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = {
    numpy = "*",
    scipy = "*", # scientific
}
"#,
                DEPS,
                "scipy"
            ),
            @r#"
        dependencies = {
            numpy = "*",
        }
        "#
        );
    }

    #[test]
    fn remove_last_entry_with_brace_on_entry_line() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = { numpy = "*",
  # more penguins
  scipy = "*" }
"#,
                DEPS,
                "scipy"
            ),
            @r#"
        dependencies = { numpy = "*"
          # more penguins
        }
        "#
        );
    }

    #[test]
    fn remove_last_entry_keeps_comment_of_surviving_entry_without_trailing_comma() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = {
  numpy = "*", # the classic
  scipy = "*"
}
"#,
                DEPS,
                "scipy"
            ),
            @r#"
        dependencies = {
          numpy = "*" # the classic
        }
        "#
        );
    }

    #[test]
    fn remove_keeps_standalone_comment_line() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = {
    numpy = "*",
    # keep me
    scipy = "*",
}
"#,
                DEPS,
                "scipy"
            ),
            @r#"
        dependencies = {
            numpy = "*",
            # keep me
        }
        "#
        );
    }

    #[test]
    fn remove_middle_entry_keeps_standalone_comment_line() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = {
    numpy = "*",
    # keep me
    scipy = "*",
    pandas = "*",
}
"#,
                DEPS,
                "scipy"
            ),
            @r#"
        dependencies = {
            numpy = "*",
            # keep me
            pandas = "*",
        }
        "#
        );
    }

    #[test]
    fn remove_only_entry_leaves_empty_inline_table() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = {
    numpy = "*",
}
"#,
                DEPS,
                "numpy"
            ),
            @"dependencies = {}"
        );
    }

    #[test]
    fn remove_from_single_line_inline_table() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = { numpy = "*", scipy = "*" }
        "#,
                DEPS,
                "numpy"
            ),
            @r#"dependencies = { scipy = "*" }"#
        );
    }

    #[test]
    fn remove_from_regular_table() {
        assert_snapshot!(
            remove_in(
                r#"[dependencies]
numpy = "*"
scipy = "*"
"#,
                DEPS,
                "numpy"
            ),
            @r#"
        [dependencies]
        scipy = "*"
        "#
        );
    }

    #[test]
    fn remove_missing_key_is_a_noop() {
        let toml = r#"dependencies = {
    numpy = "*",
}
"#;
        let mut doc: DocumentMut = toml.parse().unwrap();
        let removed = remove_entry(&mut doc["dependencies"], "pandas").unwrap();
        assert!(removed.is_none());
        assert_eq!(doc.to_string(), toml);
    }

    #[test]
    fn upsert_into_non_table_is_an_error() {
        let mut doc: DocumentMut = "dependencies = 42\n".parse().unwrap();
        let value: Value = r#""*""#.parse().unwrap();
        assert!(upsert_entry(&mut doc["dependencies"], "numpy", value).is_err());
    }
}
