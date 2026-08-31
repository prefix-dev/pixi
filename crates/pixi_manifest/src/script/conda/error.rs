use std::{fmt, ops::Range, path::PathBuf, sync::Arc};

use miette::{Diagnostic, LabeledSpan, NamedSource, SourceCode};
use pixi_toml::TomlDiagnostic;
use thiserror::Error;

use crate::script::block::BlockSourceMap;

/// Errors produced while reading a `conda-script` file.
#[derive(Debug, Error, Diagnostic)]
pub enum CondaScriptError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Envelope(#[from] Box<EnvelopeError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Metadata(#[from] Box<MetadataError>),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("a file containing a conda-script block must be valid UTF-8")]
    #[diagnostic(help("the block holds TOML, which is defined to be UTF-8"))]
    Utf8(#[from] std::str::Utf8Error),

    #[error(transparent)]
    TomlEdit(#[from] toml_edit::TomlError),

    #[error("{} is already a conda-script", path.display())]
    #[diagnostic(help("the file already carries a `/// conda-script` block"))]
    AlreadyInitialized { path: PathBuf },

    #[error("conda-script blocks do not support `{key}`")]
    #[diagnostic(help("a conda-script resolves for the machine it runs on"))]
    UnsupportedEdit { key: String },
}

#[derive(Debug, Clone)]
pub(crate) enum EnvelopeErrorKind {
    Unterminated {
        opening: Range<usize>,
        broken_line: Option<Range<usize>>,
        prefix: String,
    },
    MultipleBlocks {
        first: Range<usize>,
        second: Range<usize>,
    },
    BothBlockKinds {
        conda_script: Range<usize>,
        pep723: Range<usize>,
    },
}

/// A malformed `conda-script` comment envelope.
#[derive(Debug)]
pub struct EnvelopeError {
    pub(crate) kind: EnvelopeErrorKind,
    pub(crate) source: NamedSource<Arc<str>>,
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            EnvelopeErrorKind::Unterminated { .. } => f.write_str(
                "the `/// conda-script` block has no closing `/// end-conda-script` marker",
            ),
            EnvelopeErrorKind::MultipleBlocks { .. } => {
                f.write_str("the file contains more than one conda-script block")
            }
            EnvelopeErrorKind::BothBlockKinds { .. } => f.write_str(
                "the file contains both a PEP 723 `script` block and a conda-script block",
            ),
        }
    }
}

impl std::error::Error for EnvelopeError {}

impl Diagnostic for EnvelopeError {
    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.source)
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        match &self.kind {
            EnvelopeErrorKind::Unterminated {
                broken_line: Some(_),
                prefix,
                ..
            } => Some(Box::new(format!(
                "every line of the block must start with its comment prefix {prefix:?}"
            ))),
            EnvelopeErrorKind::Unterminated { prefix, .. } => Some(Box::new(format!(
                "close the block with `{prefix}/// end-conda-script`"
            ))),
            EnvelopeErrorKind::MultipleBlocks { .. } => Some(Box::new(
                "a file may contain at most one conda-script block",
            )),
            EnvelopeErrorKind::BothBlockKinds { .. } => Some(Box::new(
                "keep either the PEP 723 block or the conda-script block, not both",
            )),
        }
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        let labels = match &self.kind {
            EnvelopeErrorKind::Unterminated {
                opening,
                broken_line,
                ..
            } => {
                let mut labels = vec![LabeledSpan::new_primary_with_span(
                    Some("the block opens here".to_owned()),
                    opening.clone(),
                )];
                if let Some(broken_line) = broken_line {
                    labels.push(LabeledSpan::new_with_span(
                        Some("this line does not start with the block's prefix".to_owned()),
                        broken_line.clone(),
                    ));
                }
                labels
            }
            EnvelopeErrorKind::MultipleBlocks { first, second } => vec![
                LabeledSpan::new_with_span(
                    Some("the first block opens here".to_owned()),
                    first.clone(),
                ),
                LabeledSpan::new_primary_with_span(
                    Some("a second block opens here".to_owned()),
                    second.clone(),
                ),
            ],
            EnvelopeErrorKind::BothBlockKinds {
                conda_script,
                pep723,
            } => vec![
                LabeledSpan::new_primary_with_span(
                    Some("the conda-script block opens here".to_owned()),
                    conda_script.clone(),
                ),
                LabeledSpan::new_with_span(
                    Some("the PEP 723 block opens here".to_owned()),
                    pep723.clone(),
                ),
            ],
        };
        Some(Box::new(labels.into_iter()))
    }
}

/// Invalid TOML inside a `conda-script` block, with spans mapped back into
/// the original file.
#[derive(Debug)]
pub struct MetadataError {
    pub(crate) error: TomlDiagnostic,
    pub(crate) source: NamedSource<Arc<str>>,
    pub(crate) source_map: BlockSourceMap,
}

impl fmt::Display for MetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for MetadataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl Diagnostic for MetadataError {
    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.error.help()
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.source)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(self.error.labels()?.map(|label| {
            let span = self.source_map.span(label.offset(), label.len(), 0);
            let text = label.label().map(str::to_owned);
            if label.primary() {
                LabeledSpan::new_primary_with_span(text, span)
            } else {
                LabeledSpan::new_with_span(text, span)
            }
        })))
    }
}
