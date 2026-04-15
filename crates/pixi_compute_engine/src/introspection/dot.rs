//! Graphviz `.dot` rendering for [`DependencyGraph`].

use std::{
    io::{self, Write},
    path::Path,
};

use super::DependencyGraph;

impl DependencyGraph {
    /// Render the snapshot to a Graphviz `.dot` file at `path`.
    pub fn write_dot(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let mut file = std::fs::File::create(path)?;
        self.write_dot_to(&mut file)
    }

    /// Render the snapshot in Graphviz `.dot` format to a writer.
    pub fn write_dot_to<W: Write>(&self, out: &mut W) -> io::Result<()> {
        writeln!(out, "digraph deps {{")?;

        for node in self.nodes() {
            writeln!(
                out,
                "    \"{}\" [label=\"{}\"];",
                escape(&node.key.to_string()),
                escape(&node.key.to_string()),
            )?;
        }

        for (parent, children) in self.edges() {
            for child in children {
                writeln!(
                    out,
                    "    \"{}\" -> \"{}\";",
                    escape(&parent.to_string()),
                    escape(&child.to_string()),
                )?;
            }
        }

        writeln!(out, "}}")?;
        Ok(())
    }
}

/// Escape characters that are unsafe inside a Graphviz quoted string.
///
/// Quotes get backslash-escaped; newlines become `\n` so multi-line
/// `Display` impls don't break the file.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}
