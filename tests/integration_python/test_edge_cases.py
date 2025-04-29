from pathlib import Path
import shutil
import subprocess
import pytest
import platform
from .common import CURRENT_PLATFORM, verify_cli_command, ExitCode


@pytest.mark.extra_slow
def test_pypi_git_deps(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test where we need to lookup recursive git dependencies and consider them first party"""
    test_data = Path(__file__).parent.parent / "data/pixi_tomls/pip_git_dep.toml"
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = test_data.read_text()
    manifest.write_text(toml)

    # Run the installation
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
    )


@pytest.mark.slow
@pytest.mark.skipif(
    not (platform.system() == "Darwin" and platform.machine() == "arm64"),
    reason="Test tailored for macOS arm so that we can get two different python interpreters",
)
def test_python_mismatch(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test pypi wheel install where the base interpreter is not the same as the target version"""
    test_data = Path(__file__).parent.parent / "data/pixi_tomls/python_mismatch.toml"
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = test_data.read_text()
    manifest.write_text(toml)

    # Install
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
    )


@pytest.mark.slow
def test_prefix_only_created_when_sdist(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path
) -> None:
    """Test that the prefix is only created when the source distribution is used"""
    test_data = Path(__file__).parent.parent / "data/pixi_tomls/two_envs_one_sdist.toml"
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = test_data.read_text()
    manifest.write_text(toml)

    # Install
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
        # We need the an uncached version, otherwise it might skip prefix creation
        env={"PIXI_CACHE_DIR": str(tmp_path)},
    )

    # Test if the `py310` prefix is created but the `py39` is not
    py39 = tmp_pixi_workspace / ".pixi" / "envs" / "py39"
    py310 = tmp_pixi_workspace / ".pixi" / "envs" / "py310"

    assert not py39.exists()
    assert py310.exists()


def test_error_on_conda_meta_file_error(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    """Break the meta file and check if the error is caught and printed to the user"""
    verify_cli_command([pixi, "init", "-c", dummy_channel_1, tmp_pixi_workspace], ExitCode.SUCCESS)

    # Install a package
    verify_cli_command([pixi, "add", "dummy-a", "--manifest-path", tmp_pixi_workspace])

    # Create a conda meta file and path with an error
    meta_file = tmp_pixi_workspace.joinpath(
        ".pixi/envs/default/conda-meta/ca-certificates-2024.12.14-hf0a4a13_0.json"
    )
    meta_file.parent.mkdir(parents=True, exist_ok=True)
    meta_file.write_text("")

    verify_cli_command(
        [pixi, "install", "--manifest-path", tmp_pixi_workspace],
        ExitCode.FAILURE,
        stderr_contains=["failed to collect prefix records", "pixi clean"],
    )


def test_cuda_on_macos(pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str) -> None:
    """Test that we can install an environment that has cuda dependencies for linux on a macOS machine. This mimics the behavior of the pytorch installation where the linux environment should have cuda but the macOS environment should not."""
    verify_cli_command([pixi, "init", tmp_pixi_workspace, "--channel", virtual_packages_channel])
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    env = {"CONDA_OVERRIDE_CUDA": "12.0"}
    # Add multiple platforms
    verify_cli_command(
        [
            pixi,
            "project",
            "platform",
            "add",
            "--manifest-path",
            manifest,
            "linux-64",
            "osx-64",
            "osx-arm64",
            "win-64",
        ],
        ExitCode.SUCCESS,
    )

    # Add system-requirement on cuda
    verify_cli_command(
        [
            pixi,
            "project",
            "system-requirements",
            "add",
            "--manifest-path",
            manifest,
            "cuda",
            "12.1",
        ],
    )

    # Add the dependency
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest, "noarch_package", "--no-install"],
        env=env,
    )

    # Install important to run on all platforms!
    # It should succeed even though we are on macOS
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        env=env,
    )

    # Add the dependency even though the system requirements can not be satisfied on the machine
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest, "no-deps", "--no-install"],
        env=env,
    )


def test_unused_strict_system_requirements(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """Setup a project with strict system requirements that are not used by any package"""
    verify_cli_command([pixi, "init", tmp_pixi_workspace, "--channel", virtual_packages_channel])
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")

    # Add system-requirement on cuda
    verify_cli_command(
        [
            pixi,
            "project",
            "system-requirements",
            "add",
            "--manifest-path",
            manifest,
            "cuda",
            "12.1",
        ],
        ExitCode.SUCCESS,
    )
    # Add non existing glibc
    verify_cli_command(
        [
            pixi,
            "project",
            "system-requirements",
            "add",
            "--manifest-path",
            manifest,
            "glibc",
            "100.2.3",
        ],
        ExitCode.SUCCESS,
    )

    # Add non existing macos
    verify_cli_command(
        [
            pixi,
            "project",
            "system-requirements",
            "add",
            "--manifest-path",
            manifest,
            "macos",
            "123.123.0",
        ],
        ExitCode.SUCCESS,
    )

    # Add the dependency even though the system requirements can not be satisfied on the machine
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest, "no-deps", "--no-install"],
        ExitCode.SUCCESS,
    )

    # Installing should succeed as there is no virtual package that requires the system requirements
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
    )

    # Activate the environment even though the machine doesn't have the system requirements
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "echo", "Hello World"],
        ExitCode.SUCCESS,
    )


@pytest.mark.skipif(
    platform.system() == "Windows",
    reason="Post-link script uses linux commands for file tasks, fails on windows. Package needs manual fixing.",
)
def test_post_link_scripts(
    pixi: Path,
    tmp_pixi_workspace: Path,
    post_link_script_channel: str,
) -> None:
    """Test that post-link scripts are run correctly"""
    verify_cli_command([pixi, "init", tmp_pixi_workspace, "--channel", post_link_script_channel])
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")

    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest, "post-link-script-package"],
        ExitCode.SUCCESS,
    )

    # Make sure script has not ran
    assert not tmp_pixi_workspace.joinpath(".pixi", "envs", "default", ".message.txt").exists()

    # Set the config to run the script
    verify_cli_command(
        [
            pixi,
            "config",
            "set",
            "--manifest-path",
            manifest,
            "--local",
            "run-post-link-scripts",
            "insecure",
        ]
    )
    verify_cli_command([pixi, "config", "list", "--manifest-path", manifest])

    # Clean env
    verify_cli_command(
        [pixi, "clean", "--manifest-path", manifest],
        ExitCode.SUCCESS,
    )

    # Install the package
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest, "-vvv"],
        ExitCode.SUCCESS,
    )

    # Make sure script has ran
    assert tmp_pixi_workspace.joinpath(".pixi", "envs", "default", ".messages.bak.txt").exists()


@pytest.mark.extra_slow
def test_build_git_source_deps(
    pixi: Path, tmp_pixi_workspace: Path, pypi_data: Path, pixi_tomls: Path
) -> None:
    """
    This one tries to build the rich example project
    """

    project = pypi_data / "rich_table"
    target_git_dir = tmp_pixi_workspace / "git_project"
    shutil.copytree(project, target_git_dir)
    shutil.rmtree(target_git_dir.joinpath(".pixi"), ignore_errors=True)

    # init it as a git repo and commit all files
    verify_cli_command(["git", "init"], cwd=target_git_dir)
    # set some identity
    verify_cli_command(["git", "config", "user.email", "some@email.com"], cwd=target_git_dir)
    verify_cli_command(["git", "config", "user.name", "some-name"], cwd=target_git_dir)

    verify_cli_command(["git", "add", "."], cwd=target_git_dir)
    verify_cli_command(["git", "commit", "-m", "initial commit"], cwd=target_git_dir)

    # extract exact commit hash that we will use
    commit_hash = verify_cli_command(
        ["git", "rev-parse", "HEAD"], cwd=target_git_dir
    ).stdout.strip()

    minimal_workspace = tmp_pixi_workspace / "pixi_with_git_pypi"
    minimal_workspace.mkdir()
    shutil.copyfile(pixi_tomls / "pypi_local_git.toml", minimal_workspace / "pixi.toml")

    # edit the minimal_workspace to include the git_project
    workspace_manifest = minimal_workspace / "pixi.toml"

    target_git_url = target_git_dir.as_uri()

    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("file://", f"git+{target_git_url}")
    )

    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("CURRENT_PLATFORM", CURRENT_PLATFORM)
    )

    # install it
    verify_cli_command([pixi, "install", "--manifest-path", minimal_workspace / "pixi.toml"])

    # verify that we indeed recorded the git url with it's commit
    pixi_lock_file = minimal_workspace / "pixi.lock"

    assert f"pypi: git+{target_git_url}#{commit_hash}" in pixi_lock_file.read_text()

    # now we update source code so we can verify that
    # both pixi-git will discover a new commit
    # and pixi build will rebuild it
    rich_table = target_git_dir / "src" / "rich_table" / "__init__.py"
    rich_table.write_text(rich_table.read_text().replace("John Doe", "John Doe Jr."))
    # commit the change
    verify_cli_command(["git", "add", "."], cwd=target_git_dir)
    verify_cli_command(["git", "commit", "-m", "update John Doe"], cwd=target_git_dir)

    # extract updated commit hash that we will use
    new_commit_hash = verify_cli_command(
        ["git", "rev-parse", "HEAD"], cwd=target_git_dir
    ).stdout.strip()

    # update
    verify_cli_command([pixi, "update", "--manifest-path", minimal_workspace / "pixi.toml"])

    # verify that we indeed recorded the git url with it's commit
    pixi_lock_file = minimal_workspace / "pixi.lock"

    assert f"pypi: git+{target_git_url}#{new_commit_hash}" in pixi_lock_file.read_text()

    # run the python script to verify that new name is used
    verify_cli_command(
        [pixi, "run", "rich-example-main", "--manifest-path", minimal_workspace / "pixi.toml"],
        stdout_contains="John Doe Jr.",
        cwd=minimal_workspace,
    )


PYPROJECT_CONTENT = """
[project]
version = "0.1.0"
name = "test"
requires-python = "== 3.12"
dependencies = [
    "jinja2 @ https://files.pythonhosted.org/packages/62/a1/3d680cbfd5f4b8f15abc1d571870c5fc3e594bb582bc3b64ea099db13e56/jinja2-3.1.6-py3-none-any.whl#sha256=85ece4451f492d0c13c5dd7c13a64681a86afae63a5f347908daf103ce6d2f67"
]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.pixi.pypi-dependencies]
test = { path = ".", editable = true }

[tool.hatch.metadata]
allow-direct-references = true
"""


def test_pypi_url_fragment_in_project_deps(tmp_pixi_workspace: Path, pixi: Path) -> None:
    pyproject_path = tmp_pixi_workspace / "pyproject.toml"
    pyproject_path.write_text(PYPROJECT_CONTENT)

    src_dir = tmp_pixi_workspace / "src" / "test"
    src_dir.mkdir(parents=True, exist_ok=True)
    (src_dir / "__init__.py").touch()

    try:
        subprocess.run(
            [pixi, "install"], cwd=tmp_pixi_workspace, check=True, capture_output=True, text=True
        )
    except subprocess.CalledProcessError as e:
        pytest.fail(f"failed to solve the pypi requirements {e}", pytrace=False)


def test_pypi_url_fragment_hash_validation(tmp_pixi_workspace: Path, pixi: Path) -> None:
    """Test that we correctly validate the hash when installing packages with URL fragments."""
    correct_hash_content = """
[project]
version = "0.1.0"
name = "test"
requires-python = "== 3.12"
dependencies = [
    "jinja2 @ https://files.pythonhosted.org/packages/62/a1/3d680cbfd5f4b8f15abc1d571870c5fc3e594bb582bc3b64ea099db13e56/jinja2-3.1.6-py3-none-any.whl#sha256=85ece4451f492d0c13c5dd7c13a64681a86afae63a5f347908daf103ce6d2f67"
]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.pixi.pypi-dependencies]
test = { path = "." }

[tool.hatch.metadata]
allow-direct-references = true
"""
    pyproject_path = tmp_pixi_workspace / "pyproject.toml"
    pyproject_path.write_text(correct_hash_content)

    src_dir = tmp_pixi_workspace / "src" / "test"
    src_dir.mkdir(parents=True, exist_ok=True)
    (src_dir / "__init__.py").touch()

    verify_cli_command(
        [pixi, "install"], cwd=tmp_pixi_workspace, expected_exit_code=ExitCode.SUCCESS
    )

    verify_cli_command([pixi, "clean"], cwd=tmp_pixi_workspace, expected_exit_code=ExitCode.SUCCESS)

    incorrect_hash_content = """
[project]
version = "0.1.0"
name = "test"
requires-python = "== 3.12"
dependencies = [
    "jinja2 @ https://files.pythonhosted.org/packages/62/a1/3d680cbfd5f4b8f15abc1d571870c5fc3e594bb582bc3b64ea099db13e56/jinja2-3.1.6-py3-none-any.whl#sha256=0000000000000000000000000000000000000000000000000000000000000000"
]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.pixi.pypi-dependencies]
test = { path = "." }

[tool.hatch.metadata]
allow-direct-references = true
"""
    pyproject_path.write_text(incorrect_hash_content)

    # installation with incorrect hash should fail
    verify_cli_command(
        [pixi, "install"], cwd=tmp_pixi_workspace, expected_exit_code=ExitCode.FAILURE
    )
