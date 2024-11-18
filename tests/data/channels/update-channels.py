import subprocess
from pathlib import Path
import shutil
import tomllib
import argparse


def main() -> None:
    parser = argparse.ArgumentParser(description="Update a single channel.")
    parser.add_argument("channel", help="The channel to update")
    args = parser.parse_args()

    platforms = ["win-64", "linux-64", "osx-arm64", "osx-64"]
    mappings = tomllib.loads(Path("mappings.toml").read_text())
    channels_dir = Path("channels", args.channel)
    shutil.rmtree(channels_dir, ignore_errors=True)

    for recipe, channel in mappings.items():
        if channel == args.channel:
            print(recipe, channel)
            for platform in platforms:
                subprocess.run(
                    [
                        "rattler-build",
                        "build",
                        "--target-platform",
                        platform,
                        "--no-include-recipe",
                        "--output-dir",
                        f"channels/{channel}",
                        "--recipe",
                        f"recipes/{recipe}",
                    ],
                    check=True,
                )

    # Remove the build directory using shutil
    shutil.rmtree(channels_dir.joinpath("bld"), ignore_errors=True)


if __name__ == "__main__":
    main()
