"""Upload conda packages to prefix.dev with attestation generation."""

import argparse
import subprocess
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description="Upload conda packages to prefix.dev")
    parser.add_argument("packages_dir", type=Path, help="Directory containing .conda packages")
    parser.add_argument(
        "--channel", "-c", default="pixi-build-backends", help="Channel to upload to"
    )
    args = parser.parse_args()

    packages = list(args.packages_dir.rglob("*.conda"))
    if not packages:
        print(f"No .conda packages found in {args.packages_dir}")
        return

    for package in packages:
        print(f"Uploading {package}")
        subprocess.run(
            [
                "rattler-build",
                "upload",
                "prefix",
                "--skip-existing",
                "--channel",
                args.channel,
                str(package),
                "--generate-attestation",
            ],
            check=True,
        )


if __name__ == "__main__":
    main()
