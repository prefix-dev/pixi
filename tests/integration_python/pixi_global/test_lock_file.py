from pathlib import Path
from typing import Any, cast

import pytest
from rattler.lock import LockFile as _LockFile

from ..common import verify_cli_command

# Make pyright treat LockFile as Any so attribute access is allowed
LockFile = cast(Any, _LockFile)

MANIFEST_VERSION = 1


def parse_lockfile(path: Path) -> dict[str, Any]:
    """
    Parse the global lockfile using py-rattler.
    """
    assert path.exists(), f"Lockfile {path} should exist"

    lock_file = LockFile.from_path(path)
    data = lock_file.to_dict()

    assert isinstance(data, dict), "Lockfile decoding should produce a dictionary"
    return cast(dict[str, Any], data)


@pytest.mark.slow
def test_global_lockfile_populates_missing_envs_from_existing_prefixes(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    """
    Simulate an older pixi that only wrote lock entries for a subset of
    environments, and ensure the new code repopulates missing environments
    from existing prefixes on disk.
    """
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    lock_file_path = manifests.joinpath("pixi-global.lock")

    # Create two global environments: dummy-a and dummy-b
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-b"],
        env=env,
    )

    # Both prefixes must exist on disk. We rely on them to synthesize lock entries.
    env_a_dir = tmp_path.joinpath("envs", "dummy-a")
    env_b_dir = tmp_path.joinpath("envs", "dummy-b")
    assert env_a_dir.exists()
    assert env_b_dir.exists()

    # Load the lockfile and drop dummy-b from the environments section
    lf = LockFile.from_path(lock_file_path)
    data = lf.to_dict()

    envs = data.get("environments", {})
    assert "dummy-a" in envs
    assert "dummy-b" in envs

    # Simulate "old pixi" that only locked a subset of envs
    envs.pop("dummy-b", None)

    # Rebuild and write the truncated lockfile
    lf_truncated = LockFile.from_dict(data)
    lf_truncated.to_path(lock_file_path)

    # Sanity check: from the file, only dummy-a is now present
    truncated = LockFile.from_path(lock_file_path).to_dict()
    truncated_envs = truncated.get("environments", {})
    assert "dummy-a" in truncated_envs
    assert "dummy-b" not in truncated_envs

    # Run a command that loads the project and should call
    # populate_missing_lock_environments_from_existing_prefixes internally.
    verify_cli_command([pixi, "global", "sync"], env=env)

    # After sync, the lockfile should once again contain dummy-b, synthesized
    # from the existing dummy-b prefix on disk.
    final_lf = LockFile.from_path(lock_file_path)
    final_data = final_lf.to_dict()
    final_envs = final_data.get("environments", {})

    assert "dummy-a" in final_envs
    assert "dummy-b" in final_envs, (
        "populate_missing_lock_environments_from_existing_prefixes "
        "should reconstruct lock entries for existing prefixes"
    )
