"""Create the GitHub release with changelog notes, checksums and all artifacts.

Expects the git tag to already exist (created by create_tag.py) and GH_TOKEN to
be set. Runs after every artifact has been built and signed, so this is where
all checksums are computed - they can never be stale here.

Steps:
    1. Add install/install.sh and install/install.ps1 to the assets.
    2. Build source.tar.gz from the tagged tree.
    3. Write a <name>.sha256 next to each archive/msi/source and aggregate them
       into sha256.sum.
    4. Create the GitHub release with notes extracted from CHANGELOG.md.

Usage:
    pixi run -e release create-release --tag v0.71.0 --assets-dir staging/
"""

import argparse
import hashlib
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHANGELOG = ROOT / "CHANGELOG.md"
INSTALL_DIR = ROOT / "install"

# Artifact suffixes that get an individual .sha256 file and a sha256.sum entry.
CHECKSUM_SUFFIXES = (".tar.gz", ".zip", ".msi")


def run(cmd: list[str]) -> None:
    print(f"  -> {' '.join(cmd)}")
    subprocess.run(cmd, check=True, text=True)


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def changelog_notes(version: str) -> str:
    """Extract the section for `version` from CHANGELOG.md."""
    content = CHANGELOG.read_text()
    pattern = rf"(### \[{re.escape(version)}\].*?)(?=\n### \[|\n---|\Z)"
    match = re.search(pattern, content, re.DOTALL)
    return match.group(1).strip() if match else f"Release {version}"


def build_source_archive(tag: str, dest: Path) -> None:
    """Create a reproducible source tarball from the tagged tree."""
    run(
        [
            "git",
            "archive",
            "--format=tar.gz",
            f"--prefix=pixi-{tag.lstrip('v')}/",
            "--output",
            str(dest),
            tag,
        ]
    )


def write_checksums(assets_dir: Path) -> None:
    """Write per-asset .sha256 files and an aggregate sha256.sum."""
    artifacts = sorted(
        f for f in assets_dir.iterdir() if f.is_file() and f.name.endswith(CHECKSUM_SUFFIXES)
    )
    aggregate = []
    for artifact in artifacts:
        digest = sha256(artifact)
        (assets_dir / f"{artifact.name}.sha256").write_text(f"{digest}  {artifact.name}\n")
        aggregate.append(f"{digest}  {artifact.name}")
    if aggregate:
        (assets_dir / "sha256.sum").write_text("\n".join(aggregate) + "\n")
        print(f"Wrote sha256.sum with {len(aggregate)} entries")


def main() -> None:
    parser = argparse.ArgumentParser(description="Create the GitHub release")
    parser.add_argument("--tag", required=True, help="Release tag (e.g. v0.71.0)")
    parser.add_argument("--assets-dir", required=True, type=Path, help="Directory with artifacts")
    args = parser.parse_args()

    tag: str = args.tag
    version = tag.lstrip("v")
    assets_dir: Path = args.assets_dir

    if not assets_dir.is_dir():
        print(f"error: {assets_dir} is not a directory", file=sys.stderr)
        sys.exit(1)

    for installer in ("install.sh", "install.ps1"):
        shutil.copy2(INSTALL_DIR / installer, assets_dir / installer)

    build_source_archive(tag, assets_dir / "source.tar.gz")
    write_checksums(assets_dir)

    assets = sorted(f for f in assets_dir.iterdir() if f.is_file())
    print(f"Found {len(assets)} asset(s) for release {tag}:")
    for a in assets:
        print(f"  {a.name}")

    notes = changelog_notes(version)
    prerelease = ["--prerelease"] if any(c in version for c in ("rc", "-")) else []

    run(
        [
            "gh",
            "release",
            "create",
            tag,
            "--title",
            tag,
            "--notes",
            notes,
            *prerelease,
            *[str(a) for a in assets],
        ]
    )

    print(f"\nRelease {tag} created successfully.")


if __name__ == "__main__":
    main()
