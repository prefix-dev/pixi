import runpy
from pathlib import Path

import pytest
from rattler.lock import LockFile

from .common import CURRENT_PLATFORM, default_env_path, repo_root


def test_train_locks_without_installing(
    pixi: Path,
    tmp_path: Path,
    dummy_channel_1: str,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source_dir = tmp_path / "training-source"
    source_dir.mkdir()
    manifest = source_dir / "pixi.toml"
    manifest.write_text(f"""
[workspace]
name = "pgo-training-test"
channels = ["{dummy_channel_1}"]
platforms = ["{CURRENT_PLATFORM}"]

[dependencies]
dummy-b = "0.1.0"

[tasks]
training = "echo training"
""")
    working_dir = tmp_path / "work"
    working_dir.mkdir()
    profile_dir = tmp_path / "profiles"
    profile_dir.mkdir()
    train = runpy.run_path(str(repo_root() / "scripts" / "build_pgo.py"))["train"]
    monkeypatch.setitem(train.__globals__, "TRAINING_MANIFEST", manifest)
    monkeypatch.chdir(tmp_path)

    # Exercise every workload with the real CLI, including its argument parser.
    train(pixi, profile_dir, working_dir)

    lock = LockFile.from_path(working_dir / "pixi.lock")
    environment = lock.default_environment()
    assert environment is not None
    assert {
        str(platform): {package.name for package in packages}
        for platform, packages in environment.packages_by_platform().items()
    } == {CURRENT_PLATFORM: {"dummy-b"}}
    assert not default_env_path(working_dir).exists()
    assert not source_dir.joinpath("pixi.lock").exists()
