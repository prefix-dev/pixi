use clap::Parser;
use pixi_build_types::ProjectModel;
use schemars::schema_for;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Development tasks for the pixi workspace")]
enum Cli {
    /// Generate JSON Schema for pixi_build_types
    GenerateSchema,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli {
        Cli::GenerateSchema => generate_schema()?,
    }

    Ok(())
}

fn generate_schema() -> Result<(), Box<dyn std::error::Error>> {
    let schema = schema_for!(ProjectModel);
    let schema_json = serde_json::to_string_pretty(&schema)?;

    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(ToOwned::to_owned)
        .ok_or("Failed to find workspace root")?;

    let output_dir = workspace_root.join("schema");
    fs_err::create_dir_all(&output_dir)?;

    let output_path = output_dir.join("pixi_build_api.json");
    fs_err::write(&output_path, format!("{}\n", schema_json))?;

    println!("Schema written to {}", output_path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_is_up_to_date() {
        // Generate the schema programmatically
        let schema = schema_for!(ProjectModel);
        let generated_schema_json =
            serde_json::to_string_pretty(&schema).expect("Failed to serialize schema to JSON");

        // Find the workspace root
        let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(ToOwned::to_owned)
            .expect("Failed to find workspace root");

        // Read the committed schema file
        let schema_path = workspace_root.join("schema").join("pixi_build_api.json");
        let committed_schema_json =
            fs_err::read_to_string(&schema_path).expect("Failed to read committed schema file");

        // Compare the schemas using similar-asserts
        // Note: We need to trim the committed schema to account for trailing newline
        similar_asserts::assert_eq!(
            committed_schema_json.trim(),
            generated_schema_json,
            "The committed schema does not match the generated schema.\nPlease run `cargo xtask generate-schema` to update the schema file."
        );
    }
}
