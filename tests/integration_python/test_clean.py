from pathlib import Path
from .common import verify_cli_command


def test_pixi_clean_workspace(pixi: Path, tmp_pixi_workspace: Path) -> None:
    # 1. Initialize a project and add a dependency
    verify_cli_command([pixi, "add", "python"], cwd=tmp_pixi_workspace)

    # 2. Run a command to create the environment
    verify_cli_command([pixi, "run", "python", "-c", "print('hello')"], cwd=tmp_pixi_workspace)

    env_dir = tmp_pixi_workspace / ".pixi" / "envs"
    assert env_dir.exists(), "Environment directory should be created"

    # 3. Clean the workspace
    verify_cli_command([pixi, "clean"], cwd=tmp_pixi_workspace)

    # 4. Verify the environment is removed
    assert not env_dir.exists(), "Environment directory should be removed after pixi clean"


def test_pixi_clean_cache_yes(pixi: Path, tmp_path: Path) -> None:
    cache_dir = tmp_path / "pixi_cache"
    cache_dir.mkdir()
    env = {"PIXI_CACHE_DIR": str(cache_dir)}

    # Run clean --cache with yes flag
    verify_cli_command([pixi, "clean", "--cache", "--yes"], cwd=tmp_path, env=env)
    assert not cache_dir.exists(), "Cache directory should be removed"


def test_pixi_clean_pypi_cache(pixi: Path, tmp_path: Path) -> None:
    cache_dir = tmp_path / "pixi_cache"
    pypi_cache = cache_dir / "pypi-cache"
    pypi_cache.mkdir(parents=True)
    env = {"PIXI_CACHE_DIR": str(cache_dir)}

    verify_cli_command([pixi, "clean", "--pypi-cache"], cwd=tmp_path, env=env)
    assert not pypi_cache.exists(), "PyPI cache should be removed"
    assert cache_dir.exists(), "Main cache directory should remain"
