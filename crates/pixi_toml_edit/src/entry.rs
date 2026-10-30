use toml_edit::{Decor, InlineTable, Item, Key, Table, TableLike, Value};

use crate::{
    NotATableError,
    style::{
        DetachedSuffix, decor_prefix, decor_suffix, detach_suffix, detect_inline_table_style,
        drop_first_line_comment, indent_after_newline, merge_comments, merge_standalone_lines,
        raw_contains_newline, raw_str, split_first_line_comment, standalone_comment_lines,
        starts_on_fresh_line,
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
/// line it occupies in a multiline inline table or a regular table.
pub fn remove_entry(container: &mut Item, key: &str) -> Result<Option<Item>, NotATableError> {
    match container {
        Item::Table(table) => Ok(remove_table_entry(table, key)),
        Item::Value(Value::InlineTable(table)) => {
            Ok(remove_inline_table_entry(table, key).map(Item::Value))
        }
        _ => Err(NotATableError),
    }
}

/// See [`upsert_entry`].
pub fn upsert_table_entry(table: &mut Table, key: &str, value: Value) {
    if TableLike::get(table, key).is_some_and(is_dotted_item) {
        overwrite_dotted_entry(table, key, value);
        return;
    }
    if let Some(existing) = table.get_mut(key).and_then(Item::as_value_mut) {
        overwrite_value(existing, value);
        return;
    }
    table.insert(key, Item::Value(value));
}

/// Removes the entry under `key` from a regular table. The lines in front of
/// the entry (standalone comments and blank lines) survive: they move to the
/// entry that follows, or behind the previous entry when the removed entry
/// was the last one, or behind the table header when it was the only one.
pub fn remove_table_entry(table: &mut Table, key: &str) -> Option<Item> {
    let keys: Vec<String> = table.iter().map(|(key, _)| key.to_string()).collect();
    let position = keys.iter().position(|existing| existing == key)?;

    let removed_prefix = entry_prefix(table, key);
    if !removed_prefix.trim().is_empty() {
        // Only entries that render inside the table body can carry the
        // comments; sub-tables render as their own sections elsewhere.
        let next = keys[position + 1..].iter().find(|next| {
            TableLike::get(table, next).is_some_and(|item| item.is_value() || is_dotted_item(item))
        });
        let previous = keys[..position].iter().rev().find(|previous| {
            TableLike::get(table, previous)
                .is_some_and(|item| item.is_value() || is_dotted_item(item))
        });
        // The final line of the prefix is the removed entry's own line; the
        // block in front of it is what must survive.
        let block = removed_prefix
            .rsplit_once('\n')
            .map(|(head, _)| head)
            .unwrap_or("");
        if let Some(next) = next {
            let next_prefix = entry_prefix(table, next);
            set_entry_prefix(table, next, format!("{removed_prefix}{next_prefix}"));
        } else if let Some(previous) = previous {
            let separator = if starts_on_fresh_line(block) {
                ""
            } else {
                "\n"
            };
            append_entry_value_suffix(table, previous, &format!("{separator}{block}"));
        } else {
            let decor = table.decor_mut();
            let suffix = raw_as_string(decor.suffix());
            let separator = if starts_on_fresh_line(block) {
                ""
            } else {
                "\n"
            };
            decor.set_suffix(format!("{suffix}{separator}{block}"));
        }
    }

    table.remove(key)
}

/// See [`upsert_entry`].
pub fn upsert_inline_table_entry(table: &mut InlineTable, key: &str, value: Value) {
    if TableLike::get(table, key).is_some_and(is_dotted_item) {
        overwrite_dotted_entry(table, key, value);
        return;
    }
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
    // Standalone comment lines in front of the closing brace keep their own
    // lines: they move into the new entry's prefix, so the entry goes in
    // below them.
    let mut detached = detach_last_value_suffix(table);
    let mut trailing = raw_str(table.trailing()).to_string();
    if let Some((trailing_comment, rest)) = split_first_line_comment(&trailing) {
        detached.comment = merge_comments(detached.comment, Some(trailing_comment));
        trailing = rest;
    }
    if let Some(standalone) = standalone_comment_lines(&trailing) {
        let indent = indent_after_newline(&trailing).unwrap_or_default();
        let standalone = standalone.to_string();
        trailing = format!("\n{indent}");
        detached.standalone = merge_standalone_lines(detached.standalone, Some(standalone));
    }
    table.set_trailing(trailing);

    let prefix = style.new_entry_prefix(&detached);
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
    // removed entry keep their own lines, so they move along instead, and so
    // do standalone lines held in the removed value's suffix (they occur when
    // there is no trailing comma).
    let removed_prefix = entry_prefix(table, key);
    let removed_suffix_rest = drop_first_line_comment(&entry_value_suffix(table, key));
    let standalone = merge_standalone_lines(
        standalone_comment_lines(&removed_prefix).map(str::to_string),
        standalone_comment_lines(&removed_suffix_rest).map(str::to_string),
    );
    let indent = indent_after_newline(&removed_prefix).unwrap_or_default();
    if let Some(next_key) = keys.get(position + 1) {
        {
            let prefix = entry_prefix(table, next_key);
            let mut new_prefix = drop_first_line_comment(&prefix);
            if let Some(standalone) = &standalone {
                // Whatever follows the standalone lines must start on a fresh
                // line, or it would be swallowed by the comment.
                if !starts_on_fresh_line(&new_prefix) {
                    new_prefix = format!("\n{indent}");
                }
                new_prefix = format!("{standalone}{new_prefix}");
            }
            set_entry_prefix(table, next_key, new_prefix);
        }
    } else {
        let trailing = raw_str(table.trailing()).to_string();
        let mut new_trailing = drop_first_line_comment(&trailing);
        if let Some(standalone) = &standalone {
            // The closing brace must start on a fresh line, or it would be
            // swallowed by the comment.
            if !starts_on_fresh_line(&new_trailing) {
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

/// The first key in a table or inline table for which the predicate returns
/// `true`. Callers use this to find an entry under a normalized name when
/// the document spells the key differently (e.g. "hTTPx" vs "httpx").
pub fn find_table_key(item: &Item, mut predicate: impl FnMut(&str) -> bool) -> Option<String> {
    item.as_table_like().and_then(|table| {
        table
            .iter()
            .find_map(|(key, _)| predicate(key).then(|| key.to_string()))
    })
}

/// Replaces `existing` with `value` while keeping the decor, so comments and
/// spacing around the value survive the overwrite.
fn overwrite_value(existing: &mut Value, mut value: Value) {
    *value.decor_mut() = existing.decor().clone();
    *existing = value;
}

/// Whether the item is a dotted entry (`a.b = 1`). Dotted entries render
/// inside the table body, but their line decor lives on the leaf key and
/// leaf value instead of the root segment.
fn is_dotted_item(item: &Item) -> bool {
    match item {
        Item::Table(table) => table.is_dotted(),
        Item::Value(Value::InlineTable(table)) => table.is_dotted(),
        _ => false,
    }
}

/// The decor prefix rendered in front of the entry under `key`. For a dotted
/// entry this is the leaf key's decor of the first rendered line.
fn entry_prefix(table: &dyn TableLike, key: &str) -> String {
    if let Some(item) = table.get(key)
        && is_dotted_item(item)
        && let Some(inner) = item.as_table_like()
    {
        if let Some(first_key) = inner.iter().next().map(|(key, _)| key.to_string()) {
            return entry_prefix(inner, &first_key);
        }
        return String::new();
    }
    table
        .key(key)
        .map(|key| decor_prefix(key.leaf_decor()).to_string())
        .unwrap_or_default()
}

/// Sets the decor prefix rendered in front of the entry under `key`,
/// descending into dotted entries like [`entry_prefix`].
fn set_entry_prefix(table: &mut dyn TableLike, key: &str, prefix: String) {
    let dotted = table.get(key).is_some_and(is_dotted_item);
    if dotted {
        if let Some(inner) = table.get_mut(key).and_then(Item::as_table_like_mut) {
            let first_key = inner.iter().next().map(|(key, _)| key.to_string());
            if let Some(first_key) = first_key {
                set_entry_prefix(inner, &first_key, prefix);
            }
        }
        return;
    }
    if let Some(mut key) = table.key_mut(key) {
        key.leaf_decor_mut().set_prefix(prefix);
    }
}

/// The suffix decor rendered behind the entry's value. For a dotted entry
/// this is the suffix of the last rendered leaf value.
fn entry_value_suffix(table: &dyn TableLike, key: &str) -> String {
    if let Some(item) = table.get(key)
        && is_dotted_item(item)
        && let Some(inner) = item.as_table_like()
    {
        if let Some(last_key) = inner.iter().last().map(|(key, _)| key.to_string()) {
            return entry_value_suffix(inner, &last_key);
        }
        return String::new();
    }
    table
        .get(key)
        .and_then(Item::as_value)
        .map(|value| decor_suffix(value.decor()).to_string())
        .unwrap_or_default()
}

/// Detaches the suffix decor behind the entry's value, descending into
/// dotted entries like [`entry_value_suffix`]. Returns the comments carried
/// by the suffix, if any.
fn detach_entry_value_suffix(table: &mut dyn TableLike, key: &str) -> DetachedSuffix {
    let dotted = table.get(key).is_some_and(is_dotted_item);
    if dotted {
        if let Some(inner) = table.get_mut(key).and_then(Item::as_table_like_mut)
            && let Some(last_key) = inner.iter().last().map(|(key, _)| key.to_string())
        {
            return detach_entry_value_suffix(inner, &last_key);
        }
        return DetachedSuffix::default();
    }
    match table.get_mut(key).and_then(Item::as_value_mut) {
        Some(value) => detach_suffix(value.decor_mut()),
        None => DetachedSuffix::default(),
    }
}

/// Appends `addition` to the suffix decor behind the entry's value,
/// descending into dotted entries like [`entry_value_suffix`].
fn append_entry_value_suffix(table: &mut dyn TableLike, key: &str, addition: &str) {
    let dotted = table.get(key).is_some_and(is_dotted_item);
    if dotted {
        if let Some(inner) = table.get_mut(key).and_then(Item::as_table_like_mut) {
            let last_key = inner.iter().last().map(|(key, _)| key.to_string());
            if let Some(last_key) = last_key {
                append_entry_value_suffix(inner, &last_key, addition);
            }
        }
        return;
    }
    if let Some(value) = table.get_mut(key).and_then(Item::as_value_mut) {
        let suffix = decor_suffix(value.decor()).to_string();
        value.decor_mut().set_suffix(format!("{suffix}{addition}"));
    }
}

/// The suffix decor of the entry's key (the spacing in front of the `=`).
/// For a dotted entry this is the leaf key's suffix.
fn entry_key_suffix(table: &dyn TableLike, key: &str) -> String {
    if let Some(item) = table.get(key)
        && is_dotted_item(item)
        && let Some(inner) = item.as_table_like()
    {
        if let Some(last_key) = inner.iter().last().map(|(key, _)| key.to_string()) {
            return entry_key_suffix(inner, &last_key);
        }
        return String::from(" ");
    }
    table
        .key(key)
        .map(|key| decor_suffix(key.leaf_decor()).to_string())
        .unwrap_or_else(|| String::from(" "))
}

/// Overwrites a dotted entry (`d.e = 2`) with a plain `key = value` entry
/// while keeping its position, line and surrounding comments. The line decor
/// moves from the leaf key and leaf value onto the root key and new value,
/// which is where it renders once the entry is no longer dotted.
fn overwrite_dotted_entry(table: &mut dyn TableLike, key: &str, value: Value) {
    let prefix = entry_prefix(table, key);
    let value_suffix = entry_value_suffix(table, key);
    let key_suffix = entry_key_suffix(table, key);
    let Some((mut key_mut, item)) = table.get_key_value_mut(key) else {
        return;
    };
    *item = Item::Value(value.decorated(" ", value_suffix));
    let decor = key_mut.leaf_decor_mut();
    decor.set_prefix(prefix);
    decor.set_suffix(key_suffix);
}

fn detach_last_value_suffix(table: &mut InlineTable) -> DetachedSuffix {
    match table.iter().last().map(|(key, _)| key.to_string()) {
        Some(last_key) => detach_entry_value_suffix(table, &last_key),
        None => DetachedSuffix::default(),
    }
}

fn last_value_suffix_has_newline(table: &InlineTable) -> bool {
    table
        .iter()
        .last()
        .is_some_and(|(key, _)| entry_value_suffix(table, key).contains('\n'))
}

fn raw_as_string(raw: Option<&toml_edit::RawString>) -> String {
    raw.and_then(toml_edit::RawString::as_str)
        .unwrap_or("")
        .to_string()
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
    fn remove_before_dotted_key_drops_line_comment() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = {
    numpy = "*", # gone
    scipy.version = "*",
}
"#,
                DEPS,
                "numpy"
            ),
            @r#"
        dependencies = {
            scipy.version = "*",
        }
        "#
        );
    }

    #[test]
    fn remove_keeps_standalone_comment_before_dotted_key() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = {
    # keep me
    numpy = "*",
    scipy.version = "*",
}
"#,
                DEPS,
                "numpy"
            ),
            @r#"
        dependencies = {
            # keep me
            scipy.version = "*",
        }
        "#
        );
    }

    #[test]
    fn overwrite_dotted_entry_keeps_line_and_comments() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = {
    numpy = "*", # the classic
    scipy.version = "*",
    pandas = "*",
}
"#,
                DEPS,
                "scipy",
                r#"">=1.16""#
            ),
            @r#"
        dependencies = {
            numpy = "*", # the classic
            scipy = ">=1.16",
            pandas = "*",
        }
        "#
        );
    }

    #[test]
    fn remove_last_entry_keeps_standalone_comment_in_suffix() {
        assert_snapshot!(
            remove_in(
                r#"dependencies = {
    numpy = "*",
    scipy = "*" # gone
    # keep me
}
"#,
                DEPS,
                "scipy"
            ),
            @r#"
        dependencies = {
            numpy = "*"
            # keep me
        }
        "#
        );
    }

    #[test]
    fn remove_from_regular_table_keeps_standalone_comment() {
        assert_snapshot!(
            remove_in(
                r#"[dependencies]
# core scientific stack
numpy = "*"
scipy = "*"
"#,
                DEPS,
                "numpy"
            ),
            @r#"
        [dependencies]
        # core scientific stack
        scipy = "*"
        "#
        );
    }

    #[test]
    fn remove_last_regular_entry_moves_comment_behind_previous_entry() {
        assert_snapshot!(
            remove_in(
                r#"[dependencies]
numpy = "*"
# gets adopted
scipy = "*"
"#,
                DEPS,
                "scipy"
            ),
            @r#"
        [dependencies]
        numpy = "*"
        # gets adopted
        "#
        );
    }

    #[test]
    fn remove_only_regular_entry_moves_comment_behind_header() {
        assert_snapshot!(
            remove_in(
                r#"[dependencies]
# orphaned
numpy = "*"
"#,
                DEPS,
                "numpy"
            ),
            @r#"
        [dependencies]
        # orphaned
        "#
        );
    }

    // `toml_edit` normalizes all line endings to LF when serializing, so the
    // expected output is the LF spelling of the CRLF input.
    #[test]
    fn remove_keeps_surviving_comments_with_crlf() {
        let toml = "dependencies = {\r\n    numpy = \"*\",\r\n    # transitional\r\n    scipy = \"*\",\r\n    # keep me: core dep\r\n    pandas = \"*\",\r\n}\r\n";
        assert_eq!(
            remove_in(toml, DEPS, "scipy"),
            "dependencies = {\n    numpy = \"*\",\n    # transitional\n    # keep me: core dep\n    pandas = \"*\",\n}\n"
        );
    }

    #[test]
    fn insert_keeps_standalone_comment_in_suffix() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = {
    numpy = "*"
    # add the ml stack next
}
"#,
                DEPS,
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"
        dependencies = {
            numpy = "*",
            # add the ml stack next
            pydantic = ">=2,<3",
        }
        "#
        );
    }

    #[test]
    fn insert_keeps_standalone_comment_in_trailing() {
        assert_snapshot!(
            upsert_in(
                r#"dependencies = {
    numpy = "*",
    # add the ml stack next
}
"#,
                DEPS,
                "pydantic",
                r#"">=2,<3""#
            ),
            @r#"
        dependencies = {
            numpy = "*",
            # add the ml stack next
            pydantic = ">=2,<3",
        }
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
