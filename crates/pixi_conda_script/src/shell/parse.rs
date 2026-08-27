use std::{iter::Peekable, str::CharIndices};

use miette::{Diagnostic, SourceSpan};
use thiserror::Error;

use super::{Command, CommandSequence, QuotedPart, Word, WordPart};

/// A syntax error in an entrypoint command string.
///
/// The label spans the offending part of the command string; attach the
/// string with [`miette::Report::with_source_code`] to render it.
#[derive(Debug, Clone, Error, Diagnostic)]
#[error("{kind}")]
pub struct ShellParseError {
    /// What is wrong.
    pub kind: ShellParseErrorKind,
    /// Where in the command string.
    #[label("{kind}")]
    pub span: SourceSpan,
}

/// The kinds of entrypoint syntax errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ShellParseErrorKind {
    #[error("`$` must be followed by `{{NAME}}` or `(`")]
    BareDollar,

    #[error("`${{...}}` must contain a variable name of letters, digits and underscores")]
    InvalidVariableName,

    #[error("unterminated `${{...}}`")]
    UnterminatedVariable,

    #[error("unterminated `$(...)`")]
    UnterminatedSubstitution,

    #[error("unterminated single-quoted string")]
    UnterminatedSingleQuote,

    #[error("unterminated double-quoted string")]
    UnterminatedDoubleQuote,

    #[error("`&` is only valid as the `&&` sequencer")]
    LoneAmpersand,

    #[error("pipes are not supported in a conda-script entrypoint")]
    Pipe,

    #[error("`;` is not supported in a conda-script entrypoint; sequence commands with `&&`")]
    Semicolon,

    #[error("redirects are not supported in a conda-script entrypoint")]
    Redirect,

    #[error("backticks are not supported in a conda-script entrypoint; substitute with `$(...)`")]
    Backtick,

    #[error("expected a command")]
    ExpectedCommand,

    #[error("unexpected `)`")]
    UnexpectedParen,

    #[error("`$(...)` substitutions nest too deeply")]
    TooDeeplyNested,
}

/// The maximum `$(...)` nesting depth; parsing recurses per level, so an
/// unbounded depth would overflow the stack instead of reporting an error.
const MAX_SUBSTITUTION_DEPTH: usize = 64;

/// Parses an entrypoint command string into a command sequence.
pub fn parse_sequence(input: &str) -> Result<CommandSequence, ShellParseError> {
    let mut parser = Parser {
        input,
        chars: input.char_indices().peekable(),
    };
    let sequence = parser.sequence(None, 0)?;
    Ok(sequence)
}

struct Parser<'a> {
    input: &'a str,
    chars: Peekable<CharIndices<'a>>,
}

impl Parser<'_> {
    fn peek(&mut self) -> Option<(usize, char)> {
        self.chars.peek().copied()
    }

    fn next(&mut self) -> Option<(usize, char)> {
        self.chars.next()
    }

    fn error(&self, kind: ShellParseErrorKind, start: usize, len: usize) -> ShellParseError {
        ShellParseError {
            kind,
            span: SourceSpan::new(start.into(), len),
        }
    }

    /// Parses commands until the end of input, or until `)` when
    /// `substitution_start` marks an enclosing `$(`.
    fn sequence(
        &mut self,
        substitution_start: Option<usize>,
        depth: usize,
    ) -> Result<CommandSequence, ShellParseError> {
        let mut commands = Vec::new();
        let mut words = Vec::new();
        let end;
        loop {
            while self.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                self.next();
            }
            match self.peek() {
                None => {
                    if let Some(start) = substitution_start {
                        return Err(self.error(
                            ShellParseErrorKind::UnterminatedSubstitution,
                            start,
                            2,
                        ));
                    }
                    end = self.input.len();
                    break;
                }
                Some((offset, ')')) => {
                    if substitution_start.is_none() {
                        return Err(self.error(ShellParseErrorKind::UnexpectedParen, offset, 1));
                    }
                    self.next();
                    end = offset;
                    break;
                }
                Some((offset, '&')) => {
                    self.next();
                    if self.peek().is_some_and(|(_, c)| c == '&') {
                        self.next();
                        if words.is_empty() {
                            return Err(self.error(
                                ShellParseErrorKind::ExpectedCommand,
                                offset,
                                2,
                            ));
                        }
                        commands.push(Command {
                            words: std::mem::take(&mut words),
                        });
                    } else {
                        return Err(self.error(ShellParseErrorKind::LoneAmpersand, offset, 1));
                    }
                }
                Some((offset, '|')) => {
                    return Err(self.error(ShellParseErrorKind::Pipe, offset, 1));
                }
                Some((offset, ';')) => {
                    return Err(self.error(ShellParseErrorKind::Semicolon, offset, 1));
                }
                Some((offset, '<' | '>')) => {
                    return Err(self.error(ShellParseErrorKind::Redirect, offset, 1));
                }
                Some((offset, '`')) => {
                    return Err(self.error(ShellParseErrorKind::Backtick, offset, 1));
                }
                Some(_) => words.push(self.word(depth)?),
            }
        }

        if !words.is_empty() {
            commands.push(Command { words });
        } else {
            // The sequence is empty or ends in `&&`.
            return Err(self.error(ShellParseErrorKind::ExpectedCommand, end, 0));
        }
        Ok(CommandSequence { commands })
    }

    fn word(&mut self, depth: usize) -> Result<Word, ShellParseError> {
        let mut parts = Vec::new();
        let mut literal = String::new();
        loop {
            match self.peek() {
                None => break,
                Some((_, c)) if c.is_whitespace() => break,
                Some((_, '&' | ')' | '|' | ';' | '<' | '>' | '`')) => break,
                Some((offset, '\'')) => {
                    flush(&mut parts, &mut literal);
                    self.next();
                    parts.push(WordPart::SingleQuoted(self.single_quoted(offset)?));
                }
                Some((offset, '"')) => {
                    flush(&mut parts, &mut literal);
                    self.next();
                    parts.push(WordPart::DoubleQuoted(self.double_quoted(offset, depth)?));
                }
                Some((offset, '$')) => {
                    flush(&mut parts, &mut literal);
                    self.next();
                    parts.push(match self.dollar(offset, depth)? {
                        Dollar::Variable(name) => WordPart::Variable(name),
                        Dollar::Substitution(sequence) => WordPart::Substitution(sequence),
                    });
                }
                Some((_, c)) => {
                    self.next();
                    literal.push(c);
                }
            }
        }
        flush(&mut parts, &mut literal);
        Ok(Word { parts })
    }

    fn single_quoted(&mut self, open: usize) -> Result<String, ShellParseError> {
        let mut content = String::new();
        loop {
            match self.next() {
                None => {
                    return Err(self.error(ShellParseErrorKind::UnterminatedSingleQuote, open, 1));
                }
                Some((_, '\'')) => return Ok(content),
                Some((_, c)) => content.push(c),
            }
        }
    }

    fn double_quoted(
        &mut self,
        open: usize,
        depth: usize,
    ) -> Result<Vec<QuotedPart>, ShellParseError> {
        let mut parts = Vec::new();
        let mut literal = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(self.error(ShellParseErrorKind::UnterminatedDoubleQuote, open, 1));
                }
                Some((_, '"')) => {
                    self.next();
                    break;
                }
                Some((offset, '$')) => {
                    if !literal.is_empty() {
                        parts.push(QuotedPart::Literal(std::mem::take(&mut literal)));
                    }
                    self.next();
                    parts.push(match self.dollar(offset, depth)? {
                        Dollar::Variable(name) => QuotedPart::Variable(name),
                        Dollar::Substitution(sequence) => QuotedPart::Substitution(sequence),
                    });
                }
                Some((_, c)) => {
                    self.next();
                    literal.push(c);
                }
            }
        }
        if !literal.is_empty() {
            parts.push(QuotedPart::Literal(literal));
        }
        Ok(parts)
    }

    /// Parses what follows a consumed `$` at `dollar`.
    fn dollar(&mut self, dollar: usize, depth: usize) -> Result<Dollar, ShellParseError> {
        match self.peek() {
            Some((_, '{')) => {
                self.next();
                let mut name = String::new();
                loop {
                    match self.peek() {
                        None => {
                            return Err(self.error(
                                ShellParseErrorKind::UnterminatedVariable,
                                dollar,
                                2,
                            ));
                        }
                        Some((end, '}')) => {
                            self.next();
                            if !is_valid_variable_name(&name) {
                                return Err(self.error(
                                    ShellParseErrorKind::InvalidVariableName,
                                    dollar,
                                    end + 1 - dollar,
                                ));
                            }
                            return Ok(Dollar::Variable(name));
                        }
                        Some((_, c)) => {
                            self.next();
                            name.push(c);
                        }
                    }
                }
            }
            Some((_, '(')) => {
                self.next();
                if depth >= MAX_SUBSTITUTION_DEPTH {
                    return Err(self.error(ShellParseErrorKind::TooDeeplyNested, dollar, 2));
                }
                Ok(Dollar::Substitution(
                    self.sequence(Some(dollar), depth + 1)?,
                ))
            }
            _ => Err(self.error(ShellParseErrorKind::BareDollar, dollar, 1)),
        }
    }
}

enum Dollar {
    Variable(String),
    Substitution(CommandSequence),
}

fn flush(parts: &mut Vec<WordPart>, literal: &mut String) {
    if !literal.is_empty() {
        parts.push(WordPart::Literal(std::mem::take(literal)));
    }
}

fn is_valid_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use pixi_test_utils::format_parse_error;

    use super::*;

    fn parse_err(input: &str) -> String {
        format_parse_error(input, parse_sequence(input).unwrap_err())
    }

    #[test]
    fn parses_the_canonical_compile_and_run_entrypoint() {
        let sequence = parse_sequence(
            "gcc -o ${CACHE}/main ${SCRIPT} $(pkg-config --cflags --libs glib-2.0) && ${CACHE}/main",
        )
        .unwrap();

        insta::assert_debug_snapshot!(sequence, @r#"
        CommandSequence {
            commands: [
                Command {
                    words: [
                        Word {
                            parts: [
                                Literal(
                                    "gcc",
                                ),
                            ],
                        },
                        Word {
                            parts: [
                                Literal(
                                    "-o",
                                ),
                            ],
                        },
                        Word {
                            parts: [
                                Variable(
                                    "CACHE",
                                ),
                                Literal(
                                    "/main",
                                ),
                            ],
                        },
                        Word {
                            parts: [
                                Variable(
                                    "SCRIPT",
                                ),
                            ],
                        },
                        Word {
                            parts: [
                                Substitution(
                                    CommandSequence {
                                        commands: [
                                            Command {
                                                words: [
                                                    Word {
                                                        parts: [
                                                            Literal(
                                                                "pkg-config",
                                                            ),
                                                        ],
                                                    },
                                                    Word {
                                                        parts: [
                                                            Literal(
                                                                "--cflags",
                                                            ),
                                                        ],
                                                    },
                                                    Word {
                                                        parts: [
                                                            Literal(
                                                                "--libs",
                                                            ),
                                                        ],
                                                    },
                                                    Word {
                                                        parts: [
                                                            Literal(
                                                                "glib-2.0",
                                                            ),
                                                        ],
                                                    },
                                                ],
                                            },
                                        ],
                                    },
                                ),
                            ],
                        },
                    ],
                },
                Command {
                    words: [
                        Word {
                            parts: [
                                Variable(
                                    "CACHE",
                                ),
                                Literal(
                                    "/main",
                                ),
                            ],
                        },
                    ],
                },
            ],
        }
        "#);
    }

    #[test]
    fn quotes_group_and_suppress() {
        let sequence =
            parse_sequence(r#"run '${SCRIPT} literal' "quoted ${VAR} $(inner cmd)" tail"#).unwrap();

        insta::assert_debug_snapshot!(sequence, @r#"
        CommandSequence {
            commands: [
                Command {
                    words: [
                        Word {
                            parts: [
                                Literal(
                                    "run",
                                ),
                            ],
                        },
                        Word {
                            parts: [
                                SingleQuoted(
                                    "${SCRIPT} literal",
                                ),
                            ],
                        },
                        Word {
                            parts: [
                                DoubleQuoted(
                                    [
                                        Literal(
                                            "quoted ",
                                        ),
                                        Variable(
                                            "VAR",
                                        ),
                                        Literal(
                                            " ",
                                        ),
                                        Substitution(
                                            CommandSequence {
                                                commands: [
                                                    Command {
                                                        words: [
                                                            Word {
                                                                parts: [
                                                                    Literal(
                                                                        "inner",
                                                                    ),
                                                                ],
                                                            },
                                                            Word {
                                                                parts: [
                                                                    Literal(
                                                                        "cmd",
                                                                    ),
                                                                ],
                                                            },
                                                        ],
                                                    },
                                                ],
                                            },
                                        ),
                                    ],
                                ),
                            ],
                        },
                        Word {
                            parts: [
                                Literal(
                                    "tail",
                                ),
                            ],
                        },
                    ],
                },
            ],
        }
        "#);
    }

    #[test]
    fn ampersands_split_commands_without_surrounding_whitespace() {
        let sequence = parse_sequence("a&&b").unwrap();
        assert_eq!(sequence.commands.len(), 2);
        assert_eq!(
            sequence.commands[1].words[0].parts,
            [WordPart::Literal("b".to_owned())]
        );
    }

    #[test]
    fn rejects_a_bare_dollar() {
        insta::assert_snapshot!(parse_err("echo $HOME"), @"
         × `$` must be followed by `{NAME}` or `(`
          ╭─[pixi.toml:1:6]
        1 │ echo $HOME
          ·      ┬
          ·      ╰── `$` must be followed by `{NAME}` or `(`
          ╰────
        ");
    }

    #[test]
    fn rejects_an_invalid_variable_name() {
        insta::assert_snapshot!(parse_err("echo ${1BAD}"), @"
         × `${...}` must contain a variable name of letters, digits and underscores
          ╭─[pixi.toml:1:6]
        1 │ echo ${1BAD}
          ·      ───┬───
          ·         ╰── `${...}` must contain a variable name of letters, digits and underscores
          ╰────
        ");
    }

    #[test]
    fn rejects_unterminated_constructs() {
        insta::assert_snapshot!(parse_err("echo ${HOME"), @"
         × unterminated `${...}`
          ╭─[pixi.toml:1:6]
        1 │ echo ${HOME
          ·      ─┬
          ·       ╰── unterminated `${...}`
          ╰────
        ");
        insta::assert_snapshot!(parse_err("echo $(inner"), @"
         × unterminated `$(...)`
          ╭─[pixi.toml:1:6]
        1 │ echo $(inner
          ·      ─┬
          ·       ╰── unterminated `$(...)`
          ╰────
        ");
        insta::assert_snapshot!(parse_err("echo 'open"), @"
         × unterminated single-quoted string
          ╭─[pixi.toml:1:6]
        1 │ echo 'open
          ·      ┬
          ·      ╰── unterminated single-quoted string
          ╰────
        ");
        insta::assert_snapshot!(parse_err("echo \"open"), @r#"
         × unterminated double-quoted string
          ╭─[pixi.toml:1:6]
        1 │ echo "open
          ·      ┬
          ·      ╰── unterminated double-quoted string
          ╰────
        "#);
    }

    #[test]
    fn rejects_everything_outside_the_grammar() {
        insta::assert_snapshot!(parse_err("a | b"), @"
         × pipes are not supported in a conda-script entrypoint
          ╭─[pixi.toml:1:3]
        1 │ a | b
          ·   ┬
          ·   ╰── pipes are not supported in a conda-script entrypoint
          ╰────
        ");
        insta::assert_snapshot!(parse_err("a || b"), @"
         × pipes are not supported in a conda-script entrypoint
          ╭─[pixi.toml:1:3]
        1 │ a || b
          ·   ┬
          ·   ╰── pipes are not supported in a conda-script entrypoint
          ╰────
        ");
        insta::assert_snapshot!(parse_err("a ; b"), @"
         × `;` is not supported in a conda-script entrypoint; sequence commands with `&&`
          ╭─[pixi.toml:1:3]
        1 │ a ; b
          ·   ┬
          ·   ╰── `;` is not supported in a conda-script entrypoint; sequence commands with `&&`
          ╰────
        ");
        insta::assert_snapshot!(parse_err("a > file"), @"
         × redirects are not supported in a conda-script entrypoint
          ╭─[pixi.toml:1:3]
        1 │ a > file
          ·   ┬
          ·   ╰── redirects are not supported in a conda-script entrypoint
          ╰────
        ");
        insta::assert_snapshot!(parse_err("a < file"), @"
         × redirects are not supported in a conda-script entrypoint
          ╭─[pixi.toml:1:3]
        1 │ a < file
          ·   ┬
          ·   ╰── redirects are not supported in a conda-script entrypoint
          ╰────
        ");
        insta::assert_snapshot!(parse_err("a & b"), @"
         × `&` is only valid as the `&&` sequencer
          ╭─[pixi.toml:1:3]
        1 │ a & b
          ·   ┬
          ·   ╰── `&` is only valid as the `&&` sequencer
          ╰────
        ");
        insta::assert_snapshot!(parse_err("a `b`"), @"
         × backticks are not supported in a conda-script entrypoint; substitute with `$(...)`
          ╭─[pixi.toml:1:3]
        1 │ a `b`
          ·   ┬
          ·   ╰── backticks are not supported in a conda-script entrypoint; substitute with `$(...)`
          ╰────
        ");
        insta::assert_snapshot!(parse_err("a )"), @"
         × unexpected `)`
          ╭─[pixi.toml:1:3]
        1 │ a )
          ·   ┬
          ·   ╰── unexpected `)`
          ╰────
        ");
    }

    #[test]
    fn rejects_too_deeply_nested_substitutions() {
        let mut input = String::from("echo ");
        for _ in 0..100 {
            input.push_str("$(a ");
        }
        input.push_str(&")".repeat(100));
        let error = parse_sequence(&input).unwrap_err();
        assert_eq!(error.kind, ShellParseErrorKind::TooDeeplyNested);

        let mut nested_ok = String::from("echo ");
        for _ in 0..20 {
            nested_ok.push_str("$(a ");
        }
        nested_ok.push_str(&")".repeat(20));
        assert!(parse_sequence(&nested_ok).is_ok());
    }

    #[test]
    fn rejects_empty_commands() {
        insta::assert_snapshot!(parse_err(""), @"
        × expected a command
         ╭─[pixi.toml:1:1]
         ╰────
        ");
        insta::assert_snapshot!(parse_err("a &&"), @"
         × expected a command
          ╭─[pixi.toml:1:5]
        1 │ a &&
          ╰────
        ");
        insta::assert_snapshot!(parse_err("&& a"), @"
         × expected a command
          ╭─[pixi.toml:1:1]
        1 │ && a
          · ─┬
          ·  ╰── expected a command
          ╰────
        ");
        insta::assert_snapshot!(parse_err("echo $()"), @"
         × expected a command
          ╭─[pixi.toml:1:8]
        1 │ echo $()
          ·        ▲
          ·        ╰── expected a command
          ╰────
        ");
    }
}
