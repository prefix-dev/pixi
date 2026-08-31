use std::fmt;

use toml_edit::{DocumentMut, Item, Table};

use super::{CondaScriptError, CondaScriptManifest};
use crate::toml::TomlDocument;

/// An editable `conda-script` manifest presented using `pixi.toml` semantics.
///
/// The TOML document is synthetic: it exposes the block's `channels`,
/// `[dependencies]` and `[tool.pixi.pypi-dependencies]` tables through the
/// same shape as a `pixi.toml`, which is what pixi's manifest editors
/// operate on. Rendering syncs the edits back into the comment block while
/// preserving the code around it.
#[derive(Debug, Clone)]
pub struct CondaScriptManifestDocument {
    script: CondaScriptManifest,
    document: TomlDocument,
}

impl CondaScriptManifestDocument {
    pub fn new(script: CondaScriptManifest) -> Result<Self, CondaScriptError> {
        let block = script.metadata_document()?;

        let mut document = DocumentMut::new();
        let mut workspace = Table::new();
        if let Some(channels) = block.get("channels") {
            workspace.insert("channels", channels.clone());
        }
        document.insert("workspace", Item::Table(workspace));
        if let Some(dependencies) = block.get("dependencies") {
            document.insert("dependencies", dependencies.clone());
        }
        if let Some(pypi_dependencies) = block
            .get("tool")
            .and_then(Item::as_table_like)
            .and_then(|tool| tool.get("pixi"))
            .and_then(Item::as_table_like)
            .and_then(|pixi| pixi.get("pypi-dependencies"))
        {
            document.insert("pypi-dependencies", pypi_dependencies.clone());
        }

        Ok(Self {
            script,
            document: TomlDocument::new(document),
        })
    }

    pub fn document(&self) -> &TomlDocument {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut TomlDocument {
        &mut self.document
    }

    /// Renders the edits back into the full file contents.
    ///
    /// The result is validated as a `conda-script` block, so an edit the
    /// block schema rejects (say a git spec under `[dependencies]`) errors
    /// here instead of writing an unreadable file.
    pub fn render(&self) -> Result<String, CondaScriptError> {
        let mut block = self.script.metadata_document()?;
        let document = self.document.as_document();

        match document.get("dependencies") {
            Some(dependencies) => {
                insert_if_changed(&mut block, "dependencies", dependencies);
            }
            None => {
                block.remove("dependencies");
            }
        }

        let workspace = document.get("workspace").and_then(Item::as_table_like);
        if let Some(channels) = workspace.and_then(|workspace| workspace.get("channels")) {
            // The block's `channels` only takes strings; a channel with a
            // priority arrives as a table and has no representation.
            let has_priority = channels
                .as_array()
                .is_some_and(|array| array.iter().any(|value| !value.is_str()));
            if has_priority {
                return Err(CondaScriptError::UnsupportedEdit {
                    key: "channel priorities".to_owned(),
                });
            }
            insert_if_changed(&mut block, "channels", channels);
        }
        // The editable document never starts out with `platforms`; its
        // presence means an editor tried to write a key the block cannot
        // hold.
        if workspace
            .and_then(|workspace| workspace.get("platforms"))
            .is_some()
        {
            return Err(CondaScriptError::UnsupportedEdit {
                key: "platforms".to_owned(),
            });
        }

        match document.get("pypi-dependencies") {
            Some(pypi_dependencies) => {
                let unchanged = block
                    .get("tool")
                    .and_then(Item::as_table_like)
                    .and_then(|tool| tool.get("pixi"))
                    .and_then(Item::as_table_like)
                    .and_then(|pixi| pixi.get("pypi-dependencies"))
                    .is_some_and(|existing| existing.to_string() == pypi_dependencies.to_string());
                if !unchanged {
                    insert_tool_pixi(&mut block, "pypi-dependencies", pypi_dependencies.clone());
                }
            }
            None => {
                remove_tool_pixi(&mut block, "pypi-dependencies");
            }
        }

        let rendered = self.script.render_metadata(&block);
        CondaScriptManifest::from_source(self.script.path(), rendered.as_bytes())?;
        Ok(rendered)
    }
}

impl fmt::Display for CondaScriptManifestDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render().map_err(|_| fmt::Error)?)
    }
}

/// Replaces a top-level item only when the edit actually changed it.
///
/// Re-inserting an unchanged item would drop the comments attached to its
/// key, so an edit elsewhere in the block must not touch it.
fn insert_if_changed(block: &mut DocumentMut, key: &str, item: &Item) {
    let unchanged = block
        .get(key)
        .is_some_and(|existing| existing.to_string() == item.to_string());
    if !unchanged {
        block.insert(key, item.clone());
    }
}

fn insert_tool_pixi(block: &mut DocumentMut, key: &str, item: Item) {
    if block.get("tool").is_none() {
        let mut tool = Table::new();
        tool.set_implicit(true);
        block.insert("tool", Item::Table(tool));
    }
    let Some(tool) = block.get_mut("tool").and_then(Item::as_table_like_mut) else {
        return;
    };
    if tool.get("pixi").is_none() {
        let mut pixi = Table::new();
        pixi.set_implicit(true);
        tool.insert("pixi", Item::Table(pixi));
    }
    if let Some(pixi) = tool.get_mut("pixi").and_then(Item::as_table_like_mut) {
        pixi.insert(key, item);
    }
}

fn remove_tool_pixi(block: &mut DocumentMut, key: &str) {
    let Some(tool) = block.get_mut("tool").and_then(Item::as_table_like_mut) else {
        return;
    };
    if let Some(pixi) = tool.get_mut("pixi").and_then(Item::as_table_like_mut) {
        pixi.remove(key);
        if pixi.is_empty() {
            tool.remove("pixi");
        }
    }
    if tool.is_empty() {
        block.remove("tool");
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use pixi_test_utils::format_diagnostic;

    use super::*;

    fn document(contents: &str) -> CondaScriptManifestDocument {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("example.c");
        let manifest = CondaScriptManifest::from_source(path, contents.as_bytes())
            .unwrap()
            .unwrap();
        CondaScriptManifestDocument::new(manifest).unwrap()
    }

    const SOURCE: &str = r#"#include <zlib.h>
// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "run ${SCRIPT}"
//
// [dependencies]
// gcc = "*"
// /// end-conda-script
int main(void) { return 0; }
"#;

    #[test]
    fn exposes_the_block_with_pixi_toml_semantics() {
        let document = document(SOURCE);
        insta::assert_snapshot!(document.document().to_string(), @r#"
        [workspace]
        channels = ["conda-forge"]

        [dependencies]
        gcc = "*"
        "#);
    }

    #[test]
    fn renders_dependency_edits_into_the_block() {
        let mut document = document(SOURCE);
        document
            .document_mut()
            .get_or_insert_nested_table(&["dependencies"])
            .unwrap()
            .insert("zlib", toml_edit::value("1.3.*"));
        insta::assert_snapshot!(document.render().unwrap(), @r#"
        #include <zlib.h>
        // /// conda-script
        // channels = ["conda-forge"]
        // entrypoint = "run ${SCRIPT}"
        //
        // [dependencies]
        // gcc = "*"
        // zlib = "1.3.*"
        // /// end-conda-script
        int main(void) { return 0; }
        "#);
    }

    #[test]
    fn renders_pypi_edits_under_tool_pixi() {
        let mut document = document(SOURCE);
        document
            .document_mut()
            .get_or_insert_nested_table(&["pypi-dependencies"])
            .unwrap()
            .insert("requests", toml_edit::value(">=2"));
        insta::assert_snapshot!(document.render().unwrap(), @r#"
        #include <zlib.h>
        // /// conda-script
        // channels = ["conda-forge"]
        // entrypoint = "run ${SCRIPT}"
        //
        // [dependencies]
        // gcc = "*"
        //
        // [tool.pixi.pypi-dependencies]
        // requests = ">=2"
        // /// end-conda-script
        int main(void) { return 0; }
        "#);
    }

    #[test]
    fn an_edit_elsewhere_keeps_comments_on_untouched_keys() {
        let mut document = document(
            "# /// conda-script\n# # the channels comment\n# channels = [\"conda-forge\"]\n# entrypoint = \"run ${SCRIPT}\"\n#\n# [dependencies]\n# gcc = \"*\"\n# /// end-conda-script\n",
        );
        document
            .document_mut()
            .get_or_insert_nested_table(&["dependencies"])
            .unwrap()
            .insert("zlib", toml_edit::value("1.3.*"));
        insta::assert_snapshot!(document.render().unwrap(), @r#"
        # /// conda-script
        # # the channels comment
        # channels = ["conda-forge"]
        # entrypoint = "run ${SCRIPT}"
        #
        # [dependencies]
        # gcc = "*"
        # zlib = "1.3.*"
        # /// end-conda-script
        "#);
    }

    #[test]
    fn rejects_edits_the_block_schema_does_not_allow() {
        let mut document = document(SOURCE);
        let mut git = toml_edit::InlineTable::new();
        git.insert("git", "https://github.com/org/repo.git".into());
        document
            .document_mut()
            .get_or_insert_nested_table(&["dependencies"])
            .unwrap()
            .insert("demo", toml_edit::value(git));
        let error = document.render().unwrap_err();
        insta::assert_snapshot!(format_diagnostic(&error), @r#"
         × Unexpected keys, expected only 'version', 'build', 'build-number', 'channel', 'subdir', 'extras', 'flags', 'md5', 'sha256', 'url', 'when'
         ╰─▶ Unexpected keys, expected only 'version', 'build', 'build-number', 'channel', 'subdir', 'extras', 'flags', 'md5', 'sha256', 'url', 'when'
          ╭─[<CARGO_ROOT>/crates/pixi_manifest/example.c:8:13]
        7 │ // gcc = "*"
        8 │ // demo = { git = "https://github.com/org/repo.git" }
          ·             ─┬─
          ·              ╰── 'git' was not expected here
        9 │ // /// end-conda-script
          ╰────
        "#);
    }
}
