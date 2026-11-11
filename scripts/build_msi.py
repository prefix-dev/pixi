"""Build the Windows MSI installer with cargo-wix and stage it for upload.

Runs cargo-wix against crates/pixi using the bundled wix/main.wxs template and
the binary already at target/<target>/release/pixi.exe (which has been
Azure-signed by this point in the release run, so the exe embedded in the MSI is
signed). The MSI itself is signed afterwards by the workflow.

Checksums are produced centrally in create_release.py, not here.

Creates:
    staging/pixi-<target>.msi

Usage:
    pixi run -e release build-msi --target x86_64-pc-windows-msvc
"""

import argparse
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PIXI_CRATE = ROOT / "crates" / "pixi"
STAGING = ROOT / "staging"


def main() -> None:
    parser = argparse.ArgumentParser(description="Build the Windows MSI")
    parser.add_argument("--target", required=True, help="Rust target triple")
    args = parser.parse_args()

    target: str = args.target

    binary = ROOT / "target" / target / "release" / "pixi.exe"
    if not binary.is_file():
        print(f"error: {binary} not found", file=sys.stderr)
        sys.exit(1)

    STAGING.mkdir(parents=True, exist_ok=True)
    output = STAGING / f"pixi-{target}.msi"

    cmd = [
        "cargo",
        "wix",
        "--package",
        "pixi",
        "--no-build",
        "--nocapture",
        "--target",
        target,
        "--output",
        str(output),
    ]
    print(f"  -> {' '.join(cmd)} (cwd={PIXI_CRATE})")
    subprocess.run(cmd, check=True, cwd=PIXI_CRATE, text=True)

    print(f"MSI: {output.name}")


if __name__ == "__main__":
    main()
