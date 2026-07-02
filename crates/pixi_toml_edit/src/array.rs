use toml_edit::{Array, Value};

use crate::style::{
    decor_prefix, decor_suffix, detach_suffix, detect_array_style, drop_first_line_comment,
    merge_comments, raw_contains_newline, raw_str, split_first_line_comment,
    standalone_comment_lines,
};

/// Appends `value` to the array, mimicking its layout: in a multiline array
/// the element goes on its own line, copying the indentation of the existing
/// elements and keeping a trailing comma behind the last element. Single-line
/// arrays get the default `toml_edit` formatting.
pub fn push_array_element(array: &mut Array, value: Value) {
    let style = detect_array_style(array);
    if !style.is_multiline() {
        array.push(value);
        return;
    }

    // A comment on the previously last element's line is stored behind that
    // element, either in its suffix or (behind the trailing comma) in the
    // array's trailing decor. Detach it and re-attach it in front of the new
    // element's line break so it stays on the line it was written on.
    let mut comment = detach_last_element_suffix(array);
    if let Some((trailing_comment, rest)) = split_first_line_comment(raw_str(array.trailing())) {
        comment = merge_comments(comment, Some(trailing_comment));
        array.set_trailing(rest);
    }

    let prefix = style.new_entry_prefix(comment.as_deref());
    array.push_formatted(value.decorated(prefix, ""));
    array.set_trailing_comma(true);
    if !raw_contains_newline(array.trailing()) {
        array.set_trailing("\n");
    }
}

/// Inserts `value` at `index`, mimicking the array layout like
/// [`push_array_element`]. Inserting at or beyond the end behaves like a
/// push.
pub fn insert_array_element(array: &mut Array, index: usize, value: Value) {
    if index >= array.len() {
        push_array_element(array, value);
        return;
    }

    let style = detect_array_style(array);
    if style.is_multiline() {
        array.insert_formatted(index, value.decorated(style.new_entry_prefix(None), ""));
        return;
    }

    // Give the new element the spacing of the element it displaces, and make
    // sure the displaced element is separated from the comma by a space.
    let displaced_prefix = array
        .get(index)
        .map(|displaced| decor_prefix(displaced.decor()).to_string())
        .unwrap_or_default();
    array.insert_formatted(index, value.decorated(displaced_prefix, ""));
    if let Some(displaced) = array.get_mut(index + 1)
        && decor_prefix(displaced.decor()).is_empty()
    {
        displaced.decor_mut().set_prefix(" ");
    }
}

/// Retains only the elements for which the predicate returns `true`, removing
/// the other elements including the lines they occupy in a multiline array.
/// The surviving elements keep their formatting.
pub fn retain_array_elements(array: &mut Array, mut predicate: impl FnMut(&Value) -> bool) {
    let was_multiline = detect_array_style(array).is_multiline();
    let keep: Vec<bool> = array.iter().map(&mut predicate).collect();
    if keep.iter().all(|keep| *keep) {
        return;
    }

    // A comment on a removed element's line is stored in the decor of
    // whatever follows it: the next element's prefix, or the array's trailing
    // decor if the removed element was the last one. Drop it so it dies with
    // the line it was written on. Standalone comment lines in front of the
    // removed element keep their own lines, so they move along instead.
    // Consecutive removals accumulate: an element inherits the standalone
    // lines of its removed predecessor before being processed itself.
    for index in 0..keep.len() {
        if keep[index] {
            continue;
        }
        let standalone = array
            .get(index)
            .and_then(|removed| standalone_comment_lines(decor_prefix(removed.decor())))
            .map(str::to_string);
        if index + 1 < keep.len() {
            if let Some(next) = array.get_mut(index + 1) {
                let prefix = decor_prefix(next.decor()).to_string();
                let mut new_prefix = drop_first_line_comment(&prefix);
                if let Some(standalone) = &standalone {
                    new_prefix = format!("{standalone}{new_prefix}");
                }
                next.decor_mut().set_prefix(new_prefix);
            }
        } else {
            let trailing = raw_str(array.trailing()).to_string();
            let mut new_trailing = drop_first_line_comment(&trailing);
            if let Some(standalone) = &standalone {
                new_trailing = format!("{standalone}{new_trailing}");
            }
            array.set_trailing(new_trailing);
        }
    }

    let mut keep = keep.into_iter();
    array.retain(|_| keep.next().unwrap_or(true));

    if array.is_empty() {
        if raw_str(array.trailing()).trim().is_empty() {
            array.set_trailing("");
        }
        array.set_trailing_comma(false);
    } else if was_multiline
        && !raw_contains_newline(array.trailing())
        && !last_element_suffix_has_newline(array)
    {
        // The removed element carried the line break in front of the closing
        // bracket; put it back so the bracket stays on its own line.
        array.set_trailing("\n");
    }
}

fn detach_last_element_suffix(array: &mut Array) -> Option<String> {
    let last = array.iter_mut().last()?;
    detach_suffix(last.decor_mut())
}

fn last_element_suffix_has_newline(array: &Array) -> bool {
    array
        .iter()
        .last()
        .is_some_and(|value| decor_suffix(value.decor()).contains('\n'))
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;
    use toml_edit::{DocumentMut, Value};

    use super::{insert_array_element, push_array_element, retain_array_elements};

    fn push_in(toml: &str, key: &str, value: &str) -> String {
        let mut doc: DocumentMut = toml.parse().unwrap();
        let array = doc[key].as_array_mut().unwrap();
        let value: Value = value.parse().unwrap();
        push_array_element(array, value);
        doc.to_string()
    }

    fn retain_in(toml: &str, key: &str, removed: &str) -> String {
        let mut doc: DocumentMut = toml.parse().unwrap();
        let array = doc[key].as_array_mut().unwrap();
        retain_array_elements(array, |value| value.as_str() != Some(removed));
        doc.to_string()
    }

    #[test]
    fn push_into_single_line_array() {
        assert_snapshot!(
            push_in(
                r#"channels = ["conda-forge"]
        "#,
                "channels",
                r#""bioconda""#
            ),
            @r#"channels = ["conda-forge", "bioconda"]"#
        );
    }

    #[test]
    fn push_into_multiline_array_with_trailing_comma() {
        assert_snapshot!(
            push_in(
                r#"platforms = [
    "linux-64",
    "osx-arm64",
]
"#,
                "platforms",
                r#""win-64""#
            ),
            @r#"
        platforms = [
            "linux-64",
            "osx-arm64",
            "win-64",
        ]
        "#
        );
    }

    #[test]
    fn push_into_multiline_array_without_trailing_comma() {
        assert_snapshot!(
            push_in(
                r#"platforms = [
    "linux-64",
    "osx-arm64"
]
"#,
                "platforms",
                r#""win-64""#
            ),
            @r#"
        platforms = [
            "linux-64",
            "osx-arm64",
            "win-64",
        ]
        "#
        );
    }

    #[test]
    fn push_keeps_comment_on_last_element_line() {
        assert_snapshot!(
            push_in(
                r#"platforms = [
    "linux-64",
    "osx-arm64", # apple silicon
]
"#,
                "platforms",
                r#""win-64""#
            ),
            @r#"
        platforms = [
            "linux-64",
            "osx-arm64", # apple silicon
            "win-64",
        ]
        "#
        );
    }

    #[test]
    fn push_mimics_two_space_indent() {
        assert_snapshot!(
            push_in(
                r#"platforms = [
  "linux-64",
]
"#,
                "platforms",
                r#""win-64""#
            ),
            @r#"
        platforms = [
          "linux-64",
          "win-64",
        ]
        "#
        );
    }

    #[test]
    fn push_into_empty_array() {
        assert_snapshot!(
            push_in(
                "platforms = []\n",
                "platforms",
                r#""linux-64""#
            ),
            @r#"platforms = ["linux-64"]"#
        );
    }

    #[test]
    fn push_into_empty_multiline_array() {
        assert_snapshot!(
            push_in(
                "platforms = [\n]\n",
                "platforms",
                r#""linux-64""#
            ),
            @r#"
        platforms = [
            "linux-64",
        ]
        "#
        );
    }

    #[test]
    fn push_inline_table_element() {
        assert_snapshot!(
            push_in(
                r#"platforms = [
    "linux-64",
]
"#,
                "platforms",
                r#"{ name = "gpu", subdir = "linux-64" }"#
            ),
            @r#"
        platforms = [
            "linux-64",
            { name = "gpu", subdir = "linux-64" },
        ]
        "#
        );
    }

    #[test]
    fn insert_at_front_of_single_line_array() {
        let mut doc: DocumentMut = r#"channels = ["conda-forge", "bioconda"]
"#
        .parse()
        .unwrap();
        let array = doc["channels"].as_array_mut().unwrap();
        insert_array_element(array, 0, r#""prio""#.parse().unwrap());
        assert_snapshot!(doc.to_string(), @r#"channels = ["prio", "conda-forge", "bioconda"]"#);
    }

    #[test]
    fn insert_at_front_of_multiline_array() {
        let mut doc: DocumentMut = r#"channels = [
    "conda-forge",
    "bioconda",
]
"#
        .parse()
        .unwrap();
        let array = doc["channels"].as_array_mut().unwrap();
        insert_array_element(array, 0, r#""prio""#.parse().unwrap());
        assert_snapshot!(doc.to_string(), @r#"
        channels = [
            "prio",
            "conda-forge",
            "bioconda",
        ]
        "#);
    }

    #[test]
    fn insert_in_middle_of_single_line_array() {
        let mut doc: DocumentMut = r#"channels = ["conda-forge", "bioconda"]
"#
        .parse()
        .unwrap();
        let array = doc["channels"].as_array_mut().unwrap();
        insert_array_element(array, 1, r#""middle""#.parse().unwrap());
        assert_snapshot!(doc.to_string(), @r#"channels = ["conda-forge", "middle", "bioconda"]"#);
    }

    #[test]
    fn insert_beyond_end_pushes() {
        let mut doc: DocumentMut = r#"channels = [
    "conda-forge",
]
"#
        .parse()
        .unwrap();
        let array = doc["channels"].as_array_mut().unwrap();
        insert_array_element(array, 5, r#""bioconda""#.parse().unwrap());
        assert_snapshot!(doc.to_string(), @r#"
        channels = [
            "conda-forge",
            "bioconda",
        ]
        "#);
    }

    #[test]
    fn retain_removes_middle_element() {
        assert_snapshot!(
            retain_in(
                r#"platforms = [
    "linux-64",
    "osx-arm64",
    "win-64",
]
"#,
                "platforms",
                "osx-arm64"
            ),
            @r#"
        platforms = [
            "linux-64",
            "win-64",
        ]
        "#
        );
    }

    #[test]
    fn retain_removes_last_element() {
        assert_snapshot!(
            retain_in(
                r#"platforms = [
    "linux-64",
    "win-64",
]
"#,
                "platforms",
                "win-64"
            ),
            @r#"
        platforms = [
            "linux-64",
        ]
        "#
        );
    }

    #[test]
    fn retain_removes_last_element_without_trailing_comma() {
        assert_snapshot!(
            retain_in(
                r#"platforms = [
    "linux-64",
    "win-64"
]
"#,
                "platforms",
                "win-64"
            ),
            @r#"
        platforms = [
            "linux-64"
        ]
        "#
        );
    }

    #[test]
    fn retain_drops_comment_of_removed_element() {
        assert_snapshot!(
            retain_in(
                r#"platforms = [
    "linux-64", # penguins
    "win-64",
]
"#,
                "platforms",
                "linux-64"
            ),
            @r#"
        platforms = [
            "win-64",
        ]
        "#
        );
    }

    #[test]
    fn retain_drops_comment_of_removed_last_element() {
        assert_snapshot!(
            retain_in(
                r#"platforms = [
    "linux-64",
    "win-64", # remove me
]
"#,
                "platforms",
                "win-64"
            ),
            @r#"
        platforms = [
            "linux-64",
        ]
        "#
        );
    }

    #[test]
    fn retain_keeps_comment_of_surviving_element() {
        assert_snapshot!(
            retain_in(
                r#"platforms = [
    "linux-64", # penguins
    "win-64",
]
"#,
                "platforms",
                "win-64"
            ),
            @r#"
        platforms = [
            "linux-64", # penguins
        ]
        "#
        );
    }

    #[test]
    fn retain_keeps_standalone_comment_line() {
        assert_snapshot!(
            retain_in(
                r#"platforms = [
    "linux-64",
    # ci platforms
    "win-64",
]
"#,
                "platforms",
                "win-64"
            ),
            @r#"
        platforms = [
            "linux-64",
            # ci platforms
        ]
        "#
        );
    }

    #[test]
    fn retain_removes_consecutive_elements_with_comments() {
        let mut doc: DocumentMut = r#"platforms = [
    "linux-64",
    # group
    "osx-64", # first
    "osx-arm64", # second
    "win-64",
]
"#
        .parse()
        .unwrap();
        let array = doc["platforms"].as_array_mut().unwrap();
        retain_array_elements(array, |value| {
            value.as_str() != Some("osx-64") && value.as_str() != Some("osx-arm64")
        });
        assert_snapshot!(doc.to_string(), @r#"
        platforms = [
            "linux-64",
            # group
            "win-64",
        ]
        "#);
    }

    #[test]
    fn retain_nothing_leaves_empty_array() {
        assert_snapshot!(
            retain_in(
                r#"platforms = [
    "linux-64",
]
"#,
                "platforms",
                "linux-64"
            ),
            @"platforms = []"
        );
    }

    #[test]
    fn retain_in_single_line_array() {
        assert_snapshot!(
            retain_in(
                r#"channels = ["conda-forge", "bioconda"]
        "#,
                "channels",
                "bioconda"
            ),
            @r#"channels = ["conda-forge"]"#
        );
    }

    #[test]
    fn retain_everything_is_a_noop() {
        let toml = r#"platforms = [
    "linux-64",
]
"#;
        let mut doc: DocumentMut = toml.parse().unwrap();
        let array = doc["platforms"].as_array_mut().unwrap();
        retain_array_elements(array, |_| true);
        assert_eq!(doc.to_string(), toml);
    }
}
