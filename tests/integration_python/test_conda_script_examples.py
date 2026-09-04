"""Runs every conda-script example the documentation embeds.

The files live in `docs/source_files/conda_scripts/` and are included into
`docs/tutorials/conda_script.md`, so a failure here means the docs show a
broken example.
"""

from pathlib import Path

import pytest

from .common import CURRENT_PLATFORM, repo_root, verify_cli_command

EXAMPLES_DIR = repo_root().joinpath("docs/source_files/conda_scripts")

LINUX_ONLY = pytest.mark.skipif(
    not CURRENT_PLATFORM.startswith("linux"),
    reason="the example's toolchain packages only exist for Linux",
)
MOJO_PLATFORMS = pytest.mark.skipif(
    CURRENT_PLATFORM not in ("linux-64", "osx-arm64"),
    reason="the max channel does not serve mojo for this platform",
)

EXAMPLES = [
    pytest.param(
        "main.c",
        [
            'sha256("conda-script") = a733a69b1424e6d2f409c14dfb01c1c1558e0eb943786ea50eb04f55afe2226d'
        ],
        marks=LINUX_ONLY,
    ),
    pytest.param("main.py", ["count: 2", "name: conda-script"]),
    pytest.param("main.R", ['"name": "conda-script"', '"count": 2']),
    pytest.param(
        "main.cpp",
        ["primes: 2, 3, 5, 7, 11", "pi is roughly 3.142"],
        marks=LINUX_ONLY,
    ),
    pytest.param("main.f90", ["info = 0", "x =   1.000   3.000"], marks=LINUX_ONLY),
    pytest.param("main.mojo", ["languages: 2 first: mojo", '"count": 2'], marks=MOJO_PLATFORMS),
    pytest.param("main.cs", ['{"item":"answer","value":42}']),
    pytest.param("main.main.kts", ['"name": "kotlin"', '"year": 2011']),
    pytest.param("main.ts", ['[["a","b"],["c","d"]]']),
]


@pytest.mark.slow
@pytest.mark.parametrize(("example", "expected"), EXAMPLES)
def test_docs_example_runs(pixi: Path, example: str, expected: list[str]) -> None:
    verify_cli_command(
        [pixi, "run", "--experimental", "--script", EXAMPLES_DIR / example],
        stdout_contains=expected,
    )


def test_every_example_is_documented() -> None:
    """Each example file appears as a snippet in the tutorial, and each
    documented snippet has a test above."""
    tutorial = repo_root().joinpath("docs/tutorials/conda_script.md").read_text()
    on_disk = {path.name for path in EXAMPLES_DIR.iterdir()}
    documented = {
        line.split("conda_scripts/")[1].rstrip('"').strip()
        for line in tutorial.splitlines()
        if "conda_scripts/" in line
    }
    tested = {param.values[0] for param in EXAMPLES}
    assert on_disk == documented == tested
