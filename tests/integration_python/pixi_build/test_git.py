from pathlib import Path
import shutil

from ..common import verify_cli_command


def test_build_git_source_deps(pixi: Path, tmp_pixi_workspace: Path, build_data: Path) -> None:
    """
    This one tries to build the rich example project
    """

    project = build_data / "rich_example"
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

    minimal_workspace = tmp_pixi_workspace / "minimal_workspace"
    minimal_workspace.mkdir()
    shutil.copyfile(
        build_data / "manifests" / "workspace_git.toml", minimal_workspace / "pixi.toml"
    )

    # edit the minimal_workspace to include the git_project
    workspace_manifest = minimal_workspace / "pixi.toml"

    target_git_url = target_git_dir.as_uri()

    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("file:///", target_git_url)
    )

    # build it
    verify_cli_command([pixi, "install", "--manifest-path", minimal_workspace / "pixi.toml"])

    # verify that we indeed recorded the git url with it's commit
    pixi_lock_file = minimal_workspace / "pixi.lock"

    assert f"conda: git+{target_git_url}#{commit_hash}" in pixi_lock_file.read_text()


def test_build_git_source_deps_from_branch(
    pixi: Path, tmp_pixi_workspace: Path, build_data: Path
) -> None:
    """
    This one tries to build the rich example project
    """

    project = build_data / "rich_example"
    target_git_dir = tmp_pixi_workspace / "git_project"
    shutil.rmtree(project.joinpath(".pixi"), ignore_errors=True)
    shutil.copytree(project, target_git_dir)

    # init it as a git repo and commit all files to a test-branch
    verify_cli_command(["git", "init"], cwd=target_git_dir)
    # set some identity
    verify_cli_command(["git", "config", "user.email", "some@email.com"], cwd=target_git_dir)
    verify_cli_command(["git", "config", "user.name", "some-name"], cwd=target_git_dir)

    verify_cli_command(["git", "checkout", "-b", "test-branch"], cwd=target_git_dir)

    verify_cli_command(["git", "add", "."], cwd=target_git_dir)
    verify_cli_command(["git", "commit", "-m", "initial commit"], cwd=target_git_dir)

    # extract exact commit hash that we will use
    commit_hash = verify_cli_command(
        ["git", "rev-parse", "HEAD"], cwd=target_git_dir
    ).stdout.strip()

    minimal_workspace = tmp_pixi_workspace / "minimal_workspace"
    minimal_workspace.mkdir()
    shutil.copyfile(
        build_data / "manifests" / "workspace_git.toml", minimal_workspace / "pixi.toml"
    )

    # edit the minimal_workspace to include the git_project
    workspace_manifest = minimal_workspace / "pixi.toml"

    target_git_url = target_git_dir.as_uri()

    # Replace the rich_example entry using string manipulation
    original = '[dependencies]\nrich_example = { "git" = "file:///" }'
    replacement = '[dependencies]\nrich_example = { "git" = "file:///", "branch" = "test-branch"}'
    workspace_manifest.write_text(workspace_manifest.read_text().replace(original, replacement))
    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("file:///", target_git_url)
    )

    # build it
    verify_cli_command([pixi, "install", "--manifest-path", minimal_workspace / "pixi.toml"])

    # verify that we indeed recorded the git url with it's commit
    pixi_lock_file = minimal_workspace / "pixi.lock"

    # verify that we recorded used the branch
    assert (
        f"conda: git+{target_git_url}?branch=test-branch#{commit_hash}"
        in pixi_lock_file.read_text()
    )


def test_build_git_source_deps_from_rev(
    pixi: Path, tmp_pixi_workspace: Path, build_data: Path
) -> None:
    """
    This one tries to build the rich example project
    """

    project = build_data / "rich_example"
    target_git_dir = tmp_pixi_workspace / "git_project"
    shutil.copytree(project, target_git_dir)
    shutil.rmtree(target_git_dir.joinpath(".pixi"), ignore_errors=True)

    # init it as a git repo and commit all files to a test-branch
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

    minimal_workspace = tmp_pixi_workspace / "minimal_workspace"
    minimal_workspace.mkdir()
    shutil.copyfile(
        build_data / "manifests" / "workspace_git.toml", minimal_workspace / "pixi.toml"
    )

    # edit the minimal_workspace to include the git_project
    workspace_manifest = minimal_workspace / "pixi.toml"

    # Replace the rich_example entry using string manipulation
    original = '[dependencies]\nrich_example = { "git" = "file:///" }'
    replacement = (
        '[dependencies]\nrich_example = {{ "git" = "file:///", "rev" = "{commit_hash}" }}'.format(
            commit_hash=commit_hash[:7]
        )
    )

    target_git_url = target_git_dir.as_uri()

    workspace_manifest.write_text(workspace_manifest.read_text().replace(original, replacement))
    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("file:///", target_git_url)
    )

    # build it
    verify_cli_command([pixi, "install", "--manifest-path", minimal_workspace / "pixi.toml"])

    # verify that we indeed recorded the git url with it's commit
    pixi_lock_file = minimal_workspace / "pixi.lock"

    # verify that we recorded used rev but also the full one
    assert (
        f"conda: git+{target_git_url}?rev={commit_hash[:7]}#{commit_hash}"
        in pixi_lock_file.read_text()
    )


def test_build_git_source_deps_from_tag(
    pixi: Path, tmp_pixi_workspace: Path, build_data: Path
) -> None:
    """
    This one tries to build the rich example project
    """

    project = build_data / "rich_example"
    target_git_dir = tmp_pixi_workspace / "git_project"
    shutil.rmtree(project.joinpath(".pixi"), ignore_errors=True)
    shutil.copytree(project, target_git_dir)

    # init it as a git repo and commit all files to a tag called v1.0.0
    verify_cli_command(["git", "init"], cwd=target_git_dir)
    # set some identity
    verify_cli_command(["git", "config", "user.email", "some@email.com"], cwd=target_git_dir)
    verify_cli_command(["git", "config", "user.name", "some-name"], cwd=target_git_dir)

    verify_cli_command(["git", "add", "."], cwd=target_git_dir)
    verify_cli_command(["git", "commit", "-m", "initial commit"], cwd=target_git_dir)
    verify_cli_command(["git", "tag", "v1.0.0"], cwd=target_git_dir)

    # extract exact commit hash that we will use
    commit_hash = verify_cli_command(
        ["git", "rev-parse", "HEAD"], cwd=target_git_dir
    ).stdout.strip()

    minimal_workspace = tmp_pixi_workspace / "minimal_workspace"
    minimal_workspace.mkdir()
    shutil.copyfile(
        build_data / "manifests" / "workspace_git.toml", minimal_workspace / "pixi.toml"
    )

    # edit the minimal_workspace to include the git_project
    workspace_manifest = minimal_workspace / "pixi.toml"

    # Replace the rich_example entry using string manipulation
    original = '[dependencies]\nrich_example = { "git" = "file:///" }'
    replacement = '[dependencies]\nrich_example = { "git" = "file:///", "tag" = "v1.0.0" }'

    workspace_manifest.write_text(workspace_manifest.read_text().replace(original, replacement))
    workspace_manifest.write_text(
        workspace_manifest.read_text().replace("file:///", target_git_dir.as_uri())
    )

    # build it
    verify_cli_command([pixi, "install", "--manifest-path", minimal_workspace / "pixi.toml"])

    # verify that we indeed recorded the git url with it's commit
    pixi_lock_file = minimal_workspace / "pixi.lock"

    # verify that we recorded used rev but also the full one
    assert (
        f"conda: git+{target_git_dir.as_uri()}?tag=v1.0.0#{commit_hash}"
        in pixi_lock_file.read_text()
    )