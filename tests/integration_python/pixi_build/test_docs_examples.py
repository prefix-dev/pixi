import json
import shutil
import sys
from pathlib import Path

import pytest
from inline_snapshot import snapshot

from .common import (
    current_platform,
    get_manifest,
    repo_root,
    skip_if_current_platform_unsupported,
    verify_cli_command,
)

pytestmark = pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="Enable again as soon as pixi publish supports windows builds with multiple platforms",
)


class TestPixiBuild:
    pixi_projects_dir: Path = repo_root().joinpath("docs/source_files/pixi_workspaces/pixi_build")
    # Workspaces that are deliberately not run here
    excluded_projects: set[str] = {
        # Requires a ROS setup that is too heavy for this test
        "ros_ws",
        # Covered by test_build.py::test_workspace_variants_separate_work_directories
        "workspace_variants",
    }
    # Expected stdout of the 'start' task per workspace directory name
    expected_outputs: dict[str, str] = {
        "advanced_cpp": snapshot("3\n"),
        "cpp": snapshot("3\n"),
        "dev": snapshot("Hello, from dev-package!\n"),
        "getting_started": snapshot("""\
┏━━━━━━━━━━━━━━┳━━━━━┳━━━━━━━━━━━━━┓
┃ name         ┃ age ┃ city        ┃
┡━━━━━━━━━━━━━━╇━━━━━╇━━━━━━━━━━━━━┩
│ John Doe     │ 30  │ New York    │
│ Jane Smith   │ 25  │ Los Angeles │
│ Tim de Jager │ 35  │ Utrecht     │
└──────────────┴─────┴─────────────┘
"""),
        "python": snapshot("""\
┏━━━━━━━━━━━━━━┳━━━━━┳━━━━━━━━━━━━━┓
┃ name         ┃ age ┃ city        ┃
┡━━━━━━━━━━━━━━╇━━━━━╇━━━━━━━━━━━━━┩
│ John Doe     │ 30  │ New York    │
│ Jane Smith   │ 25  │ Los Angeles │
│ Tim de Jager │ 35  │ Utrecht     │
└──────────────┴─────┴─────────────┘
"""),
        "workspace": snapshot("""\
┏━━━━━━━━━━━━━━┳━━━━━┳━━━━━━━━━━━━━┓
┃ name         ┃ age ┃ city        ┃
┡━━━━━━━━━━━━━━╇━━━━━╇━━━━━━━━━━━━━┩
│ John Doe     │ 31  │ New York    │
│ Jane Smith   │ 26  │ Los Angeles │
│ Tim de Jager │ 36  │ Utrecht     │
└──────────────┴─────┴─────────────┘
"""),
    }

    def test_all_projects_have_expected_output(self) -> None:
        project_names = {path.name for path in self.pixi_projects_dir.iterdir() if path.is_dir()}
        covered = self.expected_outputs.keys() | self.excluded_projects
        assert project_names == covered, (
            "Every workspace in docs/source_files/pixi_workspaces/pixi_build must have an entry "
            "in expected_outputs or be listed in excluded_projects"
        )

    @pytest.mark.extra_slow
    @pytest.mark.parametrize("project_name", sorted(expected_outputs))
    def test_doc_pixi_workspaces_pixi_build(
        self,
        project_name: str,
        pixi: Path,
        tmp_pixi_workspace: Path,
    ) -> None:
        pixi_project = self.pixi_projects_dir.joinpath(project_name)

        # Remove existing .pixi folders
        shutil.rmtree(pixi_project.joinpath(".pixi"), ignore_errors=True)

        # Copy to workspace
        shutil.copytree(pixi_project, tmp_pixi_workspace, dirs_exist_ok=True)

        # Get manifest
        manifest = get_manifest(tmp_pixi_workspace)
        skip_if_current_platform_unsupported(manifest)

        # Run task 'start'
        output = verify_cli_command(
            [pixi, "run", "--locked", "--manifest-path", manifest, "start"],
        )

        assert output.stdout == self.expected_outputs[project_name]


@pytest.mark.extra_slow
@pytest.mark.parametrize(
    "pixi_project",
    [
        pytest.param(pixi_project, id=pixi_project.name)
        for pixi_project in repo_root()
        .joinpath("docs/source_files/pixi_workspaces/introduction")
        .iterdir()
    ],
)
def test_doc_pixi_workspaces_introduction(
    pixi_project: Path, pixi: Path, tmp_pixi_workspace: Path
) -> None:
    # Remove existing .pixi folders
    shutil.rmtree(pixi_project.joinpath(".pixi"), ignore_errors=True)

    # Copy to workspace
    shutil.copytree(pixi_project, tmp_pixi_workspace, dirs_exist_ok=True)

    # Get manifest
    manifest = get_manifest(tmp_pixi_workspace)
    skip_if_current_platform_unsupported(manifest)

    # Install the environment
    verify_cli_command(
        [pixi, "install", "--locked", "--manifest-path", manifest],
    )


@pytest.mark.extra_slow
@pytest.mark.timeout(400)
@pytest.mark.parametrize(
    "manifest",
    [
        pytest.param(manifest, id=manifest.stem)
        for manifest in repo_root().joinpath("docs/source_files/").glob("**/pytorch-*.toml")
    ],
)
def test_pytorch_documentation_examples(
    manifest: Path,
    pixi: Path,
    tmp_pixi_workspace: Path,
) -> None:
    # Copy the manifest to the tmp workspace
    toml = manifest.read_text()
    toml_name = "pyproject.toml" if "pyproject_tomls" in str(manifest) else "pixi.toml"
    manifest = tmp_pixi_workspace.joinpath(toml_name)
    manifest.write_text(toml)

    # These examples declare rich platforms (e.g. `linux-64-cuda-12-0`) whose
    # subdir is the base platform. Only install when the host subdir is among
    # the declared platforms; CUDA-only examples can't install on e.g. macOS.
    platform_ls = json.loads(
        verify_cli_command(
            [pixi, "project", "platform", "ls", "--json", "--manifest-path", manifest],
        ).stdout
    )
    supported_subdirs = {
        entry["subdir"] for entry in platform_ls["platforms"] if not entry.get("is_autodetected")
    }
    if current_platform() in supported_subdirs:
        verify_cli_command(
            [pixi, "install", "--manifest-path", manifest],
            env={"CONDA_OVERRIDE_CUDA": "12.0"},
        )


def test_doc_pixi_workspaces_minijinja_task_args(
    doc_pixi_workspaces: Path, pixi: Path, tmp_pixi_workspace: Path
) -> None:
    workspace_dir = doc_pixi_workspaces.joinpath("minijinja", "task_args")

    # Remove existing .pixi folders
    shutil.rmtree(workspace_dir.joinpath(".pixi"), ignore_errors=True)

    # Copy to workspace
    shutil.copytree(workspace_dir, tmp_pixi_workspace, dirs_exist_ok=True)

    # Get manifest
    manifest = get_manifest(tmp_pixi_workspace)

    # Install the environment
    tasks = (
        verify_cli_command(
            [pixi, "task", "list", "--machine-readable", "--manifest-path", manifest],
        )
        .stdout.strip()
        .split(" ")
    )

    results = {}
    for task in tasks:
        output = verify_cli_command(
            [pixi, "run", "--manifest-path", manifest, task, "hoi Ruben"],
        ).stdout

        results[task] = output

    assert results == snapshot(
        {
            "task1": "HOI RUBEN\n",
            "task2": "hoi ruben\n",
            "task3": "hoi Ruben!\n",
            "task4": "unix\n",
            "task5": """\
hoi
Ruben
""",
        }
    )


def test_docs_task_arguments_toml(pixi: Path, tmp_pixi_workspace: Path) -> None:
    # Load the manifest from docs and write it into a temp workspace
    manifest_src = repo_root().joinpath("docs/source_files/pixi_tomls/task_arguments.toml")
    toml = manifest_src.read_text()
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    manifest.write_text(toml)

    # greet: required argument
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "greet", "Alice"],
        stdout_contains="Hello, Alice!",
    )

    # build: optional arguments with defaults
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "build"],
        stdout_contains="Building my-app with development mode",
    )

    # build: optional arguments overridden
    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest,
            "build",
            "cool-app",
            "production",
        ],
        stdout_contains="Building cool-app with production mode",
    )

    # deploy: mixed required + optional (default)
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "deploy", "web"],
        stdout_contains="Deploying web to staging",
    )

    # deploy: mixed required + optional (override)
    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest,
            "deploy",
            "web",
            "production",
        ],
        stdout_contains="Deploying web to production",
    )
