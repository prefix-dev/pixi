//! The mini-shell a `conda-script` entrypoint runs in: whitespace
//! splitting, single and double quotes, `${VAR}` substitution, recursive
//! `$(...)` command substitution and `&&` sequencing. Nothing else: no
//! pipes, redirects, globbing, `||`, `;`, subshells or environment variable
//! assignments. Commands are spawned directly, never through a system shell.

mod execute;
mod parse;

pub use execute::{ShellContext, ShellExecutionError, execute_sequence};
pub use parse::{ShellParseError, ShellParseErrorKind, parse_sequence};

/// A chain of commands separated by `&&`; a failing command aborts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSequence {
    /// The commands in execution order.
    pub commands: Vec<Command>,
}

/// One command: a program and its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The whitespace-separated words; the first evaluates to the program.
    pub words: Vec<Word>,
}

/// One whitespace-delimited word, a concatenation of parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// The parts the word concatenates.
    pub parts: Vec<WordPart>,
}

/// A part of a word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WordPart {
    /// Unquoted literal text.
    Literal(String),
    /// Single-quoted text: no substitution happens inside.
    SingleQuoted(String),
    /// Double-quoted text: substitutions apply, the result stays one word.
    DoubleQuoted(Vec<QuotedPart>),
    /// A bare `${VAR}`; the value is inserted without splitting.
    Variable(String),
    /// A bare `$(...)`; the output is split into words on whitespace.
    Substitution(CommandSequence),
}

/// A part of a double-quoted string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotedPart {
    /// Literal text.
    Literal(String),
    /// A `${VAR}` substitution.
    Variable(String),
    /// A `$(...)` substitution; the output stays part of the single word.
    Substitution(CommandSequence),
}
