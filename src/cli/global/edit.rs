use pixi_core::global::Project;
use clap::Parser;
use fs_err as fs;
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

    // Make sure directory exists to avoid errors when opening the file
    let dir = manifest_path.parent().ok_or(miette::miette!(
        "Failed to get parent directory of manifest file"
    ))?;
    fs::create_dir_all(dir)
        .map_err(|e| miette::miette!("Failed to create directory for manifest file: {e}"))?;

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
