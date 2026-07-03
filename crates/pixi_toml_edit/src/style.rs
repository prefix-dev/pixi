use toml_edit::{Array, Decor, InlineTable, Key, RawString};

/// Indentation used when a multiline container has no entry to copy the
/// indentation from.
pub(crate) const DEFAULT_INDENT: &str = "    ";

/// Layout of an inline container, detected from the decor of its existing
/// entries.
pub(crate) enum ContainerStyle {
    SingleLine,
    Multiline { indent: String },
}

impl ContainerStyle {
    pub(crate) fn is_multiline(&self) -> bool {
        matches!(self, ContainerStyle::Multiline { .. })
    }

    /// The decor prefix that puts a new entry on its own line. A comment
    /// detached from the previously last entry is re-attached in front of the
    /// line break so it stays on the line it was written on.
    pub(crate) fn new_entry_prefix(&self, detached_comment: Option<&str>) -> String {
        let ContainerStyle::Multiline { indent } = self else {
            unreachable!("prefixes are only generated for multiline containers");
        };
        match detached_comment {
            Some(comment) => format!(" {comment}\n{indent}"),
            None => format!("\n{indent}"),
        }
    }
}

pub(crate) fn detect_inline_table_style(table: &InlineTable) -> ContainerStyle {
    let mut multiline = raw_contains_newline(table.trailing());
    let mut indent = None;

    for (key_path, value) in table.get_values() {
        let prefix = key_path_prefix(&key_path);
        if let Some(line_indent) = indent_after_newline(prefix) {
            multiline = true;
            indent.get_or_insert_with(|| line_indent.to_string());
        }
        if decor_suffix(value.decor()).contains('\n') {
            multiline = true;
        }
    }

    container_style(multiline, indent)
}

pub(crate) fn detect_array_style(array: &Array) -> ContainerStyle {
    let mut multiline = raw_contains_newline(array.trailing());
    let mut indent = None;

    for value in array.iter() {
        let prefix = decor_prefix(value.decor());
        if let Some(line_indent) = indent_after_newline(prefix) {
            multiline = true;
            indent.get_or_insert_with(|| line_indent.to_string());
        }
        if decor_suffix(value.decor()).contains('\n') {
            multiline = true;
        }
    }

    container_style(multiline, indent)
}

fn container_style(multiline: bool, indent: Option<String>) -> ContainerStyle {
    if multiline {
        ContainerStyle::Multiline {
            indent: indent.unwrap_or_else(|| DEFAULT_INDENT.to_string()),
        }
    } else {
        ContainerStyle::SingleLine
    }
}

/// Detaches the decor suffix of the previously last entry so the separator
/// comma ends up directly behind its value. Returns the comment carried by
/// the suffix, if any, so the caller can re-attach it behind the comma.
pub(crate) fn detach_suffix(decor: &mut Decor) -> Option<String> {
    let suffix = decor_suffix(decor).to_string();
    if suffix.is_empty() {
        return None;
    }
    decor.set_suffix("");
    let comment = suffix.trim();
    (!comment.is_empty()).then(|| comment.to_string())
}

/// Splits a raw decor string into the comment on its first line and the
/// remainder starting at the first line break. Returns `None` if the first
/// line carries no comment.
///
/// In a multiline container the text between an entry's separator comma and
/// the following line break visually belongs to that entry's line, but is
/// stored in the decor of whatever comes next. Splitting it off allows moving
/// a comment along with the line it was written on.
pub(crate) fn split_first_line_comment(raw: &str) -> Option<(String, String)> {
    let (first_line, rest) = match raw.split_once('\n') {
        Some((first_line, rest)) => (first_line, Some(rest)),
        None => (raw, None),
    };
    let comment = first_line.trim();
    if comment.is_empty() {
        return None;
    }
    let rest = match rest {
        Some(rest) => format!("\n{rest}"),
        None => String::new(),
    };
    Some((comment.to_string(), rest))
}

/// Returns the raw decor string without the comment on its first line,
/// leaving everything from the first line break untouched.
pub(crate) fn drop_first_line_comment(raw: &str) -> String {
    match split_first_line_comment(raw) {
        Some((_, rest)) => rest,
        None => raw.to_string(),
    }
}

/// The lines of an entry's decor prefix that come before the entry's own
/// line: standalone comment lines between the previous entry and this one.
/// Returns `None` if there are none, and keeps the line break in front of
/// each line but not the one behind the last.
pub(crate) fn standalone_comment_lines(prefix: &str) -> Option<&str> {
    let (head, _indent) = prefix.rsplit_once('\n')?;
    (!head.trim().is_empty()).then_some(head)
}

/// Merges the comment detached from the last entry's suffix with the comment
/// detached from the container's trailing decor.
pub(crate) fn merge_comments(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first} {second}")),
        (first, None) => first,
        (None, second) => second,
    }
}

/// The decor prefix of a (possibly dotted) key path. The prefix in front of
/// the first segment is rendered from the leaf key's decor, so that is where
/// the line break and indentation live.
fn key_path_prefix<'k>(key_path: &[&'k Key]) -> &'k str {
    let Some(leaf) = key_path.last() else {
        return "";
    };
    raw_as_str(leaf.leaf_decor().prefix())
}

/// The indentation of the line an entry starts on, or `None` if the entry
/// does not start on its own line.
pub(crate) fn indent_after_newline(prefix: &str) -> Option<&str> {
    prefix.rsplit_once('\n').map(|(_, indent)| indent)
}

pub(crate) fn decor_prefix(decor: &Decor) -> &str {
    raw_as_str(decor.prefix())
}

pub(crate) fn decor_suffix(decor: &Decor) -> &str {
    raw_as_str(decor.suffix())
}

pub(crate) fn raw_contains_newline(raw: &RawString) -> bool {
    raw.as_str().is_some_and(|s| s.contains('\n'))
}

pub(crate) fn raw_str(raw: &RawString) -> &str {
    raw.as_str().unwrap_or("")
}

fn raw_as_str(raw: Option<&RawString>) -> &str {
    raw.and_then(RawString::as_str).unwrap_or("")
}
