"""Extract the version from crates/pixi/Cargo.toml and print GITHUB_OUTPUT lines.

crates/pixi/Cargo.toml is the single source of truth for the release version;
it is bumped by scripts/release.py in the release PR.

Outputs:
    version - e.g. 0.71.0
    tag     - e.g. v0.71.0

Usage:
    pixi run -e release extract-version
"""

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CARGO_TOML = ROOT / "crates" / "pixi" / "Cargo.toml"


def main() -> None:
    with CARGO_TOML.open("rb") as f:
        data = tomllib.load(f)

    version = data.get("package", {}).get("version")
    if version is None:
        print("error: could not find package.version in crates/pixi/Cargo.toml", file=sys.stderr)
        sys.exit(1)

    print(f"version={version}")
    print(f"tag=v{version}")


if __name__ == "__main__":
    main()
