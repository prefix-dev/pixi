use std::path::Path;

use crate::common::PixiControl;
use pixi_test_utils::{MockRepoData, Package};
use rattler_conda_types::Platform;
use tempfile::TempDir;

fn platform_manifest(body: &str) -> String {
    format!(
        r#"
[workspace]
channels = ["https://prefix.dev/conda-forge"]
platforms = ["{platform}"]
preview = ["pixi-build"]

{body}
"#,
        platform = Platform::current()
    )
}

async fn assert_refuses(body: &str, expected: &str) {
    let pixi = PixiControl::from_manifest(&platform_manifest(body)).unwrap();
    let err = pixi
        .lock()
        .with_no_build(true)
        .await
        .expect_err("source dependency must fail closed");
    let message = format!("{err:?}");
    assert!(
        message.contains(expected),
        "expected {expected:?} in {message}"
    );
    assert!(
        message.contains("drop --no-build"),
        "missing hint in {message}"
    );
    assert!(
        !pixi.manifest_path().with_file_name("pixi.lock").exists(),
        "lock file must not be written"
    );
}

#[tokio::test]
async fn channel_only_lock_succeeds() {
    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("python", "3.11.0")
            .with_subdir(Platform::current())
            .finish(),
    );
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let pixi = PixiControl::new().unwrap();
    pixi.init()
        .with_local_channel(channel_dir.path())
        .await
        .unwrap();
    pixi.add("python").await.unwrap();

    let before = fs_err::read(pixi.manifest_path().with_file_name("pixi.lock")).unwrap();
    pixi.lock().with_no_build(true).await.unwrap();
    let after = fs_err::read(pixi.manifest_path().with_file_name("pixi.lock")).unwrap();
    assert!(!after.is_empty());
    assert_eq!(before, after);
}

#[tokio::test]
async fn conda_git_is_refused() {
    assert_refuses(
        r#"
[dependencies]
mypkg = { git = "https://example.com/mypkg" }
"#,
        "source dependency `mypkg` (git)",
    )
    .await;
}

#[tokio::test]
async fn conda_path_directory_is_refused() {
    assert_refuses(
        r#"
[dependencies]
mypkg = { path = "./mypkg" }
"#,
        "source dependency `mypkg` (path)",
    )
    .await;
}

#[tokio::test]
async fn conda_url_source_archive_is_refused() {
    assert_refuses(
        r#"
[dependencies]
mypkg = { url = "file:///tmp/mypkg.zip" }
"#,
        "source dependency `mypkg` (url)",
    )
    .await;
}

#[tokio::test]
async fn workspace_package_is_refused() {
    assert_refuses(
        r#"
[package]
name = "sdl_example"
version = "0.1.0"

[package.build]
backend = { name = "pixi-build-python", version = "*" }

[dependencies]
sdl_example = { path = "." }
"#,
        "workspace package `sdl_example`",
    )
    .await;
}

#[tokio::test]
async fn dev_path_is_refused() {
    assert_refuses(
        r#"
[dev]
pkg = { path = "./pkg" }
"#,
        "source dependency `pkg` (path)",
    )
    .await;
}

#[tokio::test]
async fn pypi_git_is_refused() {
    assert_refuses(
        r#"
[pypi-dependencies]
mypkg = { git = "https://example.com/mypkg" }
"#,
        "source dependency `mypkg` (git)",
    )
    .await;
}

#[tokio::test]
async fn pypi_path_directory_is_refused() {
    assert_refuses(
        r#"
[pypi-dependencies]
mypkg = { path = "./mypkg" }
"#,
        "source dependency `mypkg` (path)",
    )
    .await;
}

#[tokio::test]
async fn pypi_url_sdist_is_refused() {
    assert_refuses(
        r#"
[pypi-dependencies]
mypkg = { url = "file:///tmp/mypkg.tar.gz" }
"#,
        "source dependency `mypkg` (url)",
    )
    .await;
}

#[tokio::test]
async fn ambiguous_url_is_refused() {
    assert_refuses(
        r#"
[dependencies]
mypkg = { url = "https://example.com/mypkg" }
"#,
        "source dependency `mypkg` (url)",
    )
    .await;
}

#[tokio::test]
async fn other_environment_git_dep_is_refused() {
    assert_refuses(
        r#"
[dependencies]
python = "*"

[feature.other.dependencies]
mypkg = { git = "https://example.com/mypkg" }

[environments]
other = ["other"]
"#,
        "source dependency `mypkg` (git)",
    )
    .await;
}

#[tokio::test]
async fn conda_binary_path_is_not_refused() {
    let archive = Path::new(env!("CARGO_WORKSPACE_DIR")).join(
        "tests/data/channels/channels/shortcuts_channel_1/noarch/pixi-editor-1.0.0-h4616a5c_0.conda",
    );
    let pixi = PixiControl::from_manifest(&platform_manifest(&format!(
        r#"
[dependencies]
pixi-editor = {{ path = "{}" }}
"#,
        archive.display()
    )))
    .unwrap();
    let err = pixi.lock().with_no_build(true).await;
    if let Err(err) = err {
        let message = format!("{err:?}");
        assert!(
            !message.contains("refusing to invoke a build backend"),
            "binary archive must not be treated as a source spec: {message}"
        );
    }
}

#[tokio::test]
async fn pypi_wheel_path_is_not_refused() {
    let pixi = PixiControl::from_manifest(&platform_manifest(
        r#"
[pypi-dependencies]
mypkg = { path = "./mypkg.whl" }
"#,
    ))
    .unwrap();
    let err = pixi.lock().with_no_build(true).await;
    if let Err(err) = err {
        let message = format!("{err:?}");
        assert!(
            !message.contains("refusing to invoke a build backend"),
            "wheel path must not be treated as a source build: {message}"
        );
    }
}

#[tokio::test]
async fn unreferenced_package_table_is_not_refused() {
    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("python", "3.11.0")
            .with_subdir(Platform::current())
            .finish(),
    );
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();
    let pixi = PixiControl::new().unwrap();
    pixi.init()
        .with_local_channel(channel_dir.path())
        .await
        .unwrap();
    let manifest = pixi.manifest_contents().unwrap();
    let manifest = manifest.replace("[workspace]", "[workspace]\npreview = [\"pixi-build\"]");
    let manifest = format!(
        "{manifest}\n[package]\nname = \"unused\"\nversion = \"0.1.0\"\n\n[package.build]\nbackend = {{ name = \"pixi-build-python\", version = \"*\" }}\n"
    );
    pixi.update_manifest(&manifest).unwrap();
    pixi.lock().with_no_build(true).await.unwrap();
}

#[tokio::test]
async fn update_refuses_git_dependency() {
    let pixi = PixiControl::from_manifest(&platform_manifest(
        r#"
[dependencies]
mypkg = { git = "https://example.com/mypkg" }
"#,
    ))
    .unwrap();
    let err = pixi
        .update()
        .with_no_build(true)
        .await
        .expect_err("update must refuse");
    let message = format!("{err:?}");
    assert!(
        message.contains("source dependency `mypkg` (git)"),
        "{message}"
    );
}
