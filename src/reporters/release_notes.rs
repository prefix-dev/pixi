use console::Style;

/// Bloaty sections that we want to skip.
const SKIPPED_SECTIONS: [&str; 2] = ["New Contributors", "Download pixi"];

/// Create a new formatter and feed the given markdown string into it.
pub fn format_release_notes(markdown: &str) -> String {
    let mut formatted_release_notes = String::new();
    let mut discard_section = false;

    for line in markdown.lines() {
        let section_name = extract_section_name(line);

        if let Some(section_name) = section_name {
            // Check for prefix, since download section is followed by version number
            discard_section = SKIPPED_SECTIONS
                .iter()
                .any(|&s| section_name.starts_with(s));
        }

        if !discard_section {
            // Skip empty lines if the previous line was also empty (allowing only one empty line)
            if line.trim().is_empty() && formatted_release_notes.ends_with("\n\n") {
                continue;
            }

            color_line(line, &mut formatted_release_notes);
            formatted_release_notes.push('\n');
        }
    }
    formatted_release_notes
}

/// Check if the line starts a new markdown section.
fn extract_section_name(line: &str) -> Option<&str> {
    static HEADER_PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^ {0,3}#+\s+(.+)$").expect("Invalid regex pattern")
    });
    HEADER_PATTERN
        .captures(line)
        .and_then(|captures| captures.get(1).map(|m| m.as_str()))
}

fn color_line(line: &str, string_builder: &mut String) {
    let base_style = match line.trim().chars().next() {
        Some('#') => Style::new().cyan(),
        Some('*') | Some('-') => Style::new().yellow(),
        _ => Style::new(),
    };

    string_builder.push_str(&base_style.apply_to(line).to_string());
}

#[cfg(test)]
mod tests {

    use super::format_release_notes;

    use super::extract_section_name;

    #[test]
    pub fn test_markdown_section_detection() {
        // Test that formatter correctly identifies correct and improper markdown headers
        assert_eq!(extract_section_name("# Header 1"), Some("Header 1"));
        assert_eq!(extract_section_name("# Header#"), Some("Header#"));
        assert_eq!(extract_section_name("## Header 2"), Some("Header 2"));
        assert_eq!(extract_section_name(" ## Header 3"), Some("Header 3"));
        assert_eq!(
            extract_section_name("   ## Almost Code Block"),
            Some("Almost Code Block")
        );

        assert_eq!(extract_section_name("###Header"), None);
        assert_eq!(extract_section_name("Header 3# Header"), None);
        assert_eq!(extract_section_name("    # Code Block"), None);
    }

    #[test]
    pub fn test_compare_release_notes() {
        // Test that the formatter correctly skips sections and formats the release notes
        let markdown = r#"#### Highlights
- World peace
- Bread will no longer fall butter-side down

#### Changed
- The sky is now green
- Water is now dry

#### New Contributors
- @alice (Alice)
- @bob (Bob)

#### Download pixi v1.2.3
No one knows what a markdown table looks like by heart
Let's just say it's a table"#;

        fn append_line(expected: &mut String, line: &str, style: Option<&console::Style>) {
            let styled_line = match style {
                Some(style) => style.apply_to(line).to_string(),
                None => line.to_string(),
            };
            expected.push_str(&styled_line);
            expected.push('\n');
        }
        let mut expected = String::new();
        let yellow = &console::Style::new().yellow();
        let cyan = &console::Style::new().cyan();
        append_line(&mut expected, "#### Highlights", Some(cyan));
        append_line(&mut expected, "- World peace", Some(yellow));
        append_line(
            &mut expected,
            "- Bread will no longer fall butter-side down",
            Some(yellow),
        );
        append_line(&mut expected, "", None);
        append_line(&mut expected, "#### Changed", Some(cyan));
        append_line(&mut expected, "- The sky is now green", Some(yellow));
        append_line(&mut expected, "- Water is now dry", Some(yellow));
        append_line(&mut expected, "", None);

        let formatted = format_release_notes(markdown);

        // Ensure same number of lines (zip will stop at the shortest)
        assert_eq!(
            expected.lines().count(),
            formatted.lines().count(),
            "Line count differs"
        );

        // assert line by line to get a better error message
        for (i, (expected_line, formatted_line)) in
            expected.lines().zip(formatted.lines()).enumerate()
        {
            assert_eq!(expected_line, formatted_line, "Line {} differs", i + 1);
        }
    }
}
