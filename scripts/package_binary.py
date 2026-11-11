"""Package a release binary into an archive and stage it for upload.

The binary at `target/<target>/release/pixi[.exe]` is expected to already be
codesigned (macOS) or Azure-signed (Windows) when this script runs. The archive
contains only the bare binary at its root (named `pixi`/`pixi.exe`), matching the
layout `install/install.sh` and `install/install.ps1` expect.

Checksums are intentionally not computed here: they are produced centrally in
create_release.py after every artifact has been signed, so they never go stale.

Creates in staging/:
    pixi-<target>.tar.gz | .zip   - archive with the bare binary in its root
    pixi-<target>[.exe]           - the raw binary

Outputs:
    archive - archive filename (e.g. pixi-x86_64-unknown-linux-musl.tar.gz)

Usage:
    pixi run -e release package-binary --target x86_64-unknown-linux-musl
"""

import argparse
import os
import shutil
import sys
import tarfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
STAGING = ROOT / "staging"


def main() -> None:
    parser = argparse.ArgumentParser(description="Package a release binary")
    parser.add_argument("--target", required=True, help="Rust target triple")
    args = parser.parse_args()

    target: str = args.target
    windows = "pc-windows" in target
    ext = ".exe" if windows else ""
    archive_ext = ".zip" if windows else ".tar.gz"

    binary_src = ROOT / "target" / target / "release" / f"pixi{ext}"
    if not binary_src.is_file():
        print(f"error: {binary_src} not found", file=sys.stderr)
        sys.exit(1)

    STAGING.mkdir(parents=True, exist_ok=True)

    archive_name = f"pixi-{target}{archive_ext}"
    archive_path = STAGING / archive_name
    arcname = f"pixi{ext}"
    if windows:
        with zipfile.ZipFile(archive_path, "w", zipfile.ZIP_DEFLATED) as zf:
            zf.write(binary_src, arcname=arcname)
    else:
        with tarfile.open(archive_path, "w:gz") as tf:
            tf.add(binary_src, arcname=arcname)

    raw_binary = STAGING / f"pixi-{target}{ext}"
    shutil.copy2(binary_src, raw_binary)

    print(f"Archive: {archive_name}")
    print(f"Binary: {raw_binary.name}")

    github_output = os.environ.get("GITHUB_OUTPUT")
    if github_output:
        with Path(github_output).open("a") as f:
            f.write(f"archive={archive_name}\n")


if __name__ == "__main__":
    main()
