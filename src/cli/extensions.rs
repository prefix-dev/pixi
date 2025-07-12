use std::collections::HashMap;

use clap::Parser;
use itertools::Itertools;

use crate::cli::command_info::find_all_external_commands;

#[derive(Parser, Debug)]
#[clap(verbatim_doc_comment)]
pub struct Args;

pub async fn execute(_args: Args) -> miette::Result<()> {
    let known_extensions = HashMap::from([
        (
            "pixi-pack".to_string(),
            "Pack conda environments created with pixi".to_string(),
        ),
        (
            "pixi-unpack".to_string(),
            "Unpack conda environments packaged with pixi-pack".to_string(),
        ),
        (
            "pixi-diff".to_string(),
            "Generate JSON diffs between pixi lockfiles".to_string(),
        ),
        (
            "pixi-diff-to-markdown".to_string(),
            "Generate markdown summaries from pixi update".to_string(),
        ),
        (
            "pixi-inject".to_string(),
            "Inject conda packages into an already existing conda prefix".to_string(),
        ),
        (
            "pixi-install-to-prefix".to_string(),
            "Install pixi environments to an arbitrary prefix".to_string(),
        ),
    ]);

    let extensions = find_all_external_commands();

    println!(
        "{}",
        console::style("Available Extensions:")
            .green()
            .underlined()
            .bold()
    );

    for extension in extensions.iter().sorted() {
        let Some(name) = extension
            .as_path()
            .file_stem()
            .map(|p| p.to_string_lossy().to_string())
        else {
            continue;
        };
        print!("  {}", console::style(&name).cyan());
        if let Some(description) = known_extensions.get(&name) {
            println!("\t{}", description);
        } else {
            println!();
        }
    }

    Ok(())
}
