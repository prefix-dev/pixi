use crate::global::Project;
use clap::Parser;
use miette::IntoDiagnostic;

/// Edit the global manifest file
///
/// Opens your editor to edit the global manifest file.
#[derive(Parser, Debug)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    /// The editor to use, defaults to `EDITOR` environment variable or `nano` on Unix and `notepad` on Windows
    #[arg(env = "EDITOR")]
    pub editor: Option<String>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let manifest_path = Project::default_manifest_path()?;

    let editor = args.editor.unwrap_or_else(|| {
        if cfg!(windows) {
            "notepad".to_string()
        } else {
            "nano".to_string()
        }
    });

    let mut child = if cfg!(windows) {
        std::process::Command::new("cmd")
            .arg("/C")
            .arg(editor.as_str())
            .arg(&manifest_path)
            .spawn()
            .into_diagnostic()?
    } else {
        std::process::Command::new(editor.as_str())
            .arg(&manifest_path)
            .spawn()
            .into_diagnostic()?
    };
    child.wait().into_diagnostic()?;
    Ok(())
}
