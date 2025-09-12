import json
from pathlib import Path

from .common import EMPTY_BOILERPLATE_PROJECT, verify_cli_command


def test_explicit_manifest_correct_location(pixi: Path, tmp_path: Path) -> None:
    current_dir = tmp_path / "current"
    target_dir = tmp_path / "target"
    current_dir.mkdir()
    target_dir.mkdir()

    (current_dir / "pixi.toml").write_text(EMPTY_BOILERPLATE_PROJECT)
    (target_dir / "pixi.toml").write_text(EMPTY_BOILERPLATE_PROJECT)

    out = verify_cli_command(
        [
            pixi,
            "shell-hook",
            "--manifest-path",
            target_dir,
            "--json",
        ],
        cwd=current_dir,
    )

    payload = json.loads(out.stdout)
    value = payload["environment_variables"].get("PIXI_PROJECT_MANIFEST")
    assert value is not None, "PIXI_PROJECT_MANIFEST missing from activated env"

    expected = (target_dir / "pixi.toml").resolve()
    actual = Path(value).resolve()
    assert actual == expected
