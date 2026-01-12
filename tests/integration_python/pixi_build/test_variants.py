from pathlib import Path

import pytest
import rattler
import tomli_w
import tomllib

from .common import CURRENT_PLATFORM, copytree_with_local_backend, verify_cli_command


@pytest.mark.parametrize(
    "workspace_dirname",
    ["build-variant-manifest-rattler-build", "build-variant-manifest-python"],
)
def test_inline_variants_produce_multiple_outputs(
    pixi: Path,
    tmp_pixi_workspace: Path,
    build_data: Path,
    multiple_versions_channel_1: str,
    workspace_dirname: str,
) -> None:
    test_workspace = build_data.joinpath(workspace_dirname)
    copytree_with_local_backend(test_workspace, tmp_pixi_workspace, dirs_exist_ok=True)

    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")
    manifest = tomllib.loads(manifest_path.read_text())
    manifest["workspace"]["channels"].append(multiple_versions_channel_1)
    manifest_path.write_text(tomli_w.dumps(manifest))

    output_dir = tmp_pixi_workspace.joinpath("dist")

    verify_cli_command(
        [
            pixi,
            "build",
            "--path",
            manifest_path,
            "--output-dir",
            output_dir,
        ],
    )

    # Ensure that we don't create directories we don't need
    assert not output_dir.joinpath("noarch").exists()
    assert not output_dir.joinpath(CURRENT_PLATFORM).exists()

    # Ensure that exactly two conda packages have been built
    built_packages = list(output_dir.glob("*.conda"))
    assert len(built_packages) == 2
    for package in built_packages:
        assert package.exists()


@pytest.mark.parametrize(
    "workspace_dirname",
    ["build-variant-manifest-rattler-build", "build-variant-manifest-python"],
)
def test_inline_variants_change_triggers_rebuild(
    pixi: Path,
    tmp_pixi_workspace: Path,
    build_data: Path,
    multiple_versions_channel_1: str,
    workspace_dirname: str,
) -> None:
    test_workspace = build_data.joinpath(workspace_dirname)
    copytree_with_local_backend(test_workspace, tmp_pixi_workspace, dirs_exist_ok=True)

    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")
    manifest = tomllib.loads(manifest_path.read_text())
    manifest["workspace"]["channels"].append(multiple_versions_channel_1)
    manifest_path.write_text(tomli_w.dumps(manifest))

    verify_cli_command(
        [
            pixi,
            "install",
            "--manifest-path",
            manifest_path,
        ],
    )

    verify_cli_command(
        [pixi, "run", "package3"],
        cwd=tmp_pixi_workspace,
        stdout_contains="0.3.0",
    )

    manifest["workspace"]["build-variants"]["package3"] = ["0.2.0"]
    manifest_path.write_text(tomli_w.dumps(manifest))

    verify_cli_command(
        [pixi, "run", "package3"],
        cwd=tmp_pixi_workspace,
        stdout_contains="0.2.0",
    )


@pytest.mark.parametrize(
    "workspace_dirname",
    [
        "build-variant-files-rattler-build",
        "build-variant-files-python",
        "build-variant-conda-config-rattler-build",
        "build-variant-conda-config-python",
    ],
)
def test_variant_files_produce_multiple_outputs(
    pixi: Path,
    tmp_pixi_workspace: Path,
    build_data: Path,
    multiple_versions_channel_1: str,
    workspace_dirname: str,
) -> None:
    test_workspace = build_data.joinpath(workspace_dirname)
    copytree_with_local_backend(test_workspace, tmp_pixi_workspace, dirs_exist_ok=True)

    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")
    manifest = tomllib.loads(manifest_path.read_text())
    manifest["workspace"]["channels"].append(multiple_versions_channel_1)
    manifest_path.write_text(tomli_w.dumps(manifest))

    output_dir = tmp_pixi_workspace.joinpath("dist")

    verify_cli_command(
        [
            pixi,
            "build",
            "--path",
            manifest_path,
            "--output-dir",
            output_dir,
        ],
    )

    built_packages = list(output_dir.glob("*.conda"))

    # On unix, the variant has three entries, on windows only two
    if rattler.Platform.current().is_unix:
        assert len(built_packages) == 3
    else:
        assert len(built_packages) == 2


@pytest.mark.parametrize(
    "workspace_dirname",
    [
        "build-variant-files-rattler-build",
        "build-variant-files-python",
        "build-variant-conda-config-rattler-build",
        "build-variant-conda-config-python",
    ],
)
def test_variant_files_change_triggers_rebuild(
    pixi: Path,
    tmp_pixi_workspace: Path,
    build_data: Path,
    multiple_versions_channel_1: str,
    workspace_dirname: str,
) -> None:
    test_workspace = build_data.joinpath(workspace_dirname)
    copytree_with_local_backend(test_workspace, tmp_pixi_workspace, dirs_exist_ok=True)

    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")
    manifest = tomllib.loads(manifest_path.read_text())
    manifest["workspace"]["channels"].append(multiple_versions_channel_1)
    manifest_path.write_text(tomli_w.dumps(manifest))

    verify_cli_command(
        [
            pixi,
            "install",
            "--manifest-path",
            manifest_path,
        ],
    )

    verify_cli_command(
        [pixi, "run", "package3"],
        cwd=tmp_pixi_workspace,
        stdout_contains="0.3.0",
    )

    uses_conda_config = "conda-config" in workspace_dirname
    variant_file = tmp_pixi_workspace.joinpath(
        "corp-pinning",
        "conda_build_config.yaml" if uses_conda_config else "config.yaml",
    )

    variant_contents = variant_file.read_text()
    assert "0.3.0" in variant_contents
    variant_file.write_text(variant_contents.replace("\n  - 0.3.0", "", 1))

    verify_cli_command(
        [pixi, "run", "package3"],
        cwd=tmp_pixi_workspace,
        stdout_contains="0.2.0",
    )
