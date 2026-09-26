//! Machinery shared by the metadata block kinds a script file can carry:
//! the PEP 723 `script` block and the `conda-script` block. Both embed TOML
//! in a comment block, strip the comment prefix on read, map offsets in the
//! extracted TOML back to the original file for diagnostics, and serialize
//! edited TOML back between the block markers.

use std::ops::Range;

use miette::SourceSpan;

#[derive(Debug, Clone, Copy)]
pub(crate) enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub(crate) fn detect(contents: &[u8]) -> Self {
        if contents.windows(2).any(|window| window == b"\r\n") {
            Self::CrLf
        } else {
            Self::Lf
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}

/// One line of extracted block content: where it starts in the extracted
/// metadata and where its content starts in the original file.
#[derive(Debug, Clone)]
pub(crate) struct MetadataLine {
    pub(crate) metadata_start: usize,
    pub(crate) source_start: usize,
    pub(crate) len: usize,
}

/// Maps offsets in the extracted metadata TOML back to the original file.
#[derive(Debug, Clone)]
pub(crate) struct BlockSourceMap {
    pub(crate) opening: Range<usize>,
    pub(crate) metadata_lines: Vec<MetadataLine>,
}

impl BlockSourceMap {
    fn metadata_offset(&self, offset: usize) -> usize {
        let Some(line) = self
            .metadata_lines
            .iter()
            .rev()
            .find(|line| line.metadata_start <= offset)
        else {
            return self.opening.start;
        };
        line.source_start + offset.saturating_sub(line.metadata_start).min(line.len)
    }

    /// A span in the original file for a span in the extracted metadata.
    ///
    /// `synthetic_prefix` is the length of text prepended to the metadata
    /// before parsing; offsets inside that prefix map to the opening marker.
    pub(crate) fn span(&self, offset: usize, len: usize, synthetic_prefix: usize) -> SourceSpan {
        let Some(metadata_start) = offset.checked_sub(synthetic_prefix) else {
            return SourceSpan::from(self.opening.clone());
        };
        let metadata_end = offset.saturating_add(len).saturating_sub(synthetic_prefix);
        let start = self.metadata_offset(metadata_start);
        let end = self.metadata_offset(metadata_end).max(start);
        SourceSpan::new(start.into(), end - start)
    }
}

/// Serializes metadata TOML back into a comment block: every line carries
/// `prefix` (trimmed on empty lines), framed by the opening and closing
/// marker lines.
pub(crate) fn serialize_block(
    metadata: &str,
    prefix: &str,
    opening: &str,
    closing: &str,
    line_ending: &str,
) -> String {
    let mut output = String::with_capacity(metadata.len() + 64);
    output.push_str(opening);
    output.push_str(line_ending);
    for line in metadata.lines() {
        if line.is_empty() {
            output.push_str(prefix.trim_end());
        } else {
            output.push_str(prefix);
            output.push_str(line);
        }
        output.push_str(line_ending);
    }
    output.push_str(closing);
    output.push_str(line_ending);
    output
}

pub(crate) fn without_line_ending(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// Splits file contents into a BOM, an optional shebang line and the rest.
pub(crate) fn extract_script_header(
    contents: &[u8],
) -> Result<(&str, Option<&str>, &str), std::str::Utf8Error> {
    let contents = std::str::from_utf8(contents)?;
    let (bom, contents) = contents
        .strip_prefix('\u{feff}')
        .map_or(("", contents), |contents| ("\u{feff}", contents));
    if !contents.starts_with("#!") {
        return Ok((bom, None, contents));
    }

    let bytes = contents.as_bytes();
    let end = bytes
        .iter()
        .position(|byte| matches!(byte, b'\r' | b'\n'))
        .unwrap_or(bytes.len());
    let newline_width = match bytes.get(end..) {
        Some([b'\r', b'\n', ..]) => 2,
        Some([b'\r' | b'\n', ..]) => 1,
        _ => 0,
    };

    Ok((
        bom,
        Some(&contents[..end]),
        &contents[end + newline_width..],
    ))
}
