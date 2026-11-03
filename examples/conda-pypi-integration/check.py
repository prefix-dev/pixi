"""Smoke-test wheels installed as conda packages from the conda-pypi channel."""

from importlib.metadata import version

import black
from boltons.iterutils import chunked

assert chunked([1, 2, 3], 2) == [[1, 2], [3]]
assert black.format_str("x=1", mode=black.Mode()) == "x = 1\n"
print(f"boltons {version('boltons')} + black {version('black')} (conda-pypi wheels): OK")
