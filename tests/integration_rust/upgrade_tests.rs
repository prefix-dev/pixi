use indexmap::IndexMap;
use insta::assert_snapshot;
use pixi::Workspace;
use pixi::cli::upgrade::{Args, parse_specs};
use std::io::Write;
use std::path::Path;
use tempfile::tempdir;

// When the specific template is not in the file or the file does not exist.
// Make the file and append the template to the file.
fn create_or_append_file(path: &Path, template: &str) -> std::io::Result<()> {
    let file = fs_err::read_to_string(path).unwrap_or_default();

    if !file.contains(template) {
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)?
            .write_all(template.as_bytes())?;
    }
    Ok(())
}

// This test requires network connection and takes around 40s to
// complete on my machine.
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
#[tokio::test]
async fn pypi_dependency_index_preserved_on_upgrade() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("pixi.toml");
    let file_contents = r#"
[workspace]
channels = ["conda-forge"]
platforms = ["linux-64"]
exclude-newer = "2025-05-19"

[pypi-dependencies]
click = { version = "==8.2.0", index = "https://pypi.tuna.tsinghua.edu.cn/simple" }

[dependencies]
python = "==3.13.3""#;
    create_or_append_file(&file_path, file_contents).unwrap();

    let mut args = Args::default();
    args.workspace_config.manifest_path = Some(file_path.clone());

    let workspace = Workspace::from_path(&file_path).unwrap();

    let workspace_value = workspace.workspace.value.clone();
    let feature = workspace_value.feature(&args.specs.feature).unwrap();

    let mut workspace = workspace.modify().unwrap();

    let (match_specs, pypi_deps) = parse_specs(feature, &args, &workspace).unwrap();

    let _ = workspace
        .update_dependencies(
            match_specs,
            pypi_deps,
            IndexMap::default(),
            &args.prefix_update_config,
            &args.lock_file_update_config,
            &args.specs.feature,
            &[],
            true,
            args.dry_run,
        )
        .await
        .unwrap();

    workspace.save().await.unwrap();

    assert_snapshot!(fs_err::read_to_string(file_path).unwrap_or_default());
}
