from pathlib import Path

from .common import verify_cli_command


def test_search_MatchSpec(pixi: Path) -> None:
    verify_cli_command([pixi, "search", "python 3.12.*"])

    verify_cli_command([pixi, "search", "python-*"])
