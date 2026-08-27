use std::ops::Range;

use miette::SourceSpan;

use crate::error::EnvelopeErrorKind;

pub(crate) const OPENING_MARKER: &str = "/// conda-script";
pub(crate) const CLOSING_MARKER: &str = "/// end-conda-script";
const PEP723_OPENING: &str = "# /// script";

/// The extracted content of a `conda-script` block.
pub(crate) struct CondaScriptBlock {
    /// The block content with the comment prefix stripped from every line.
    pub(crate) metadata: String,
    pub(crate) source_map: SourceMap,
}

/// Maps offsets in the extracted metadata TOML back to the original file.
#[derive(Debug, Clone)]
pub(crate) struct SourceMap {
    opening: Range<usize>,
    metadata_lines: Vec<MetadataLine>,
}

#[derive(Debug, Clone)]
struct MetadataLine {
    metadata_start: usize,
    source_start: usize,
    len: usize,
}

impl SourceMap {
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

    pub(crate) fn span(&self, offset: usize, len: usize) -> SourceSpan {
        let start = self.metadata_offset(offset);
        let end = self.metadata_offset(offset.saturating_add(len)).max(start);
        SourceSpan::new(start.into(), end - start)
    }
}

/// Extracts the single `conda-script` block from a source file.
///
/// Returns `Ok(None)` when the file contains no opening marker with a valid
/// comment prefix.
pub(crate) fn parse_block(source: &str) -> Result<Option<CondaScriptBlock>, EnvelopeErrorKind> {
    // A BOM must not become part of the comment prefix of a block on the
    // first line.
    let base = if source.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };

    let mut lines: Vec<(usize, &str)> = Vec::new();
    let mut offset = base;
    for raw_line in source[base..].split_inclusive('\n') {
        lines.push((offset, without_line_ending(raw_line)));
        offset += raw_line.len();
    }

    let Some((opening_index, prefix)) = lines
        .iter()
        .enumerate()
        .find_map(|(index, (_, line))| opening_prefix(line).map(|prefix| (index, prefix)))
    else {
        return Ok(None);
    };
    let opening = line_span(lines[opening_index]);

    let mut toml_lines: Vec<&str> = Vec::new();
    let mut metadata_lines = Vec::new();
    let mut metadata_len = 0;
    let mut closing_index = None;
    let mut broken_line = None;
    for (index, &(line_start, line)) in lines.iter().enumerate().skip(opening_index + 1) {
        if let Some(rest) = line.strip_prefix(prefix) {
            if rest.trim_end() == CLOSING_MARKER {
                closing_index = Some(index);
                break;
            }
            toml_lines.push(rest);
            metadata_lines.push(MetadataLine {
                metadata_start: metadata_len,
                source_start: line_start + prefix.len(),
                len: rest.len(),
            });
            metadata_len += rest.len() + 1;
        } else if line.trim_end() == prefix.trim_end() {
            toml_lines.push("");
            metadata_lines.push(MetadataLine {
                metadata_start: metadata_len,
                source_start: line_start + line.trim_end().len(),
                len: 0,
            });
            metadata_len += 1;
        } else {
            broken_line = Some(line_span((line_start, line)));
            break;
        }
    }

    let Some(closing_index) = closing_index else {
        return Err(EnvelopeErrorKind::Unterminated {
            opening,
            broken_line,
            prefix: prefix.to_owned(),
        });
    };

    if let Some(second) = lines[closing_index + 1..]
        .iter()
        .find(|(_, line)| opening_prefix(line).is_some())
    {
        return Err(EnvelopeErrorKind::MultipleBlocks {
            first: opening,
            second: line_span(*second),
        });
    }

    if let Some(pep723) = lines[..opening_index]
        .iter()
        .chain(&lines[closing_index + 1..])
        .find(|(_, line)| line.trim_end() == PEP723_OPENING)
    {
        return Err(EnvelopeErrorKind::BothBlockKinds {
            conda_script: opening,
            pep723: line_span(*pep723),
        });
    }

    Ok(Some(CondaScriptBlock {
        metadata: toml_lines.join("\n") + "\n",
        source_map: SourceMap {
            opening,
            metadata_lines,
        },
    }))
}

fn line_span((start, line): (usize, &str)) -> Range<usize> {
    start..start + line.trim_end().len()
}

/// The comment prefix when `line` opens a `conda-script` block.
///
/// A prefix must be non-empty and free of alphanumeric characters, so a
/// mention of the marker inside code (`x = "// /// conda-script"`) does not
/// open a block.
fn opening_prefix(line: &str) -> Option<&str> {
    let prefix = line.trim_end().strip_suffix(OPENING_MARKER)?;
    (!prefix.is_empty() && !prefix.contains(char::is_alphanumeric)).then_some(prefix)
}

fn without_line_ending(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}
