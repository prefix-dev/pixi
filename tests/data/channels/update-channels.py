import argparse
import shutil
import subprocess
import tomllib
from pathlib import Path

import yaml
from rattler import Platform


def main() -> None:
    parser = argparse.ArgumentParser(description="Update a single channel.")
    parser.add_argument("channel", help="The channel to update")
    args = parser.parse_args()

    mappings = tomllib.loads(Path("mappings.toml").read_text())
    channels_dir = Path("channels", args.channel)
    shutil.rmtree(channels_dir, ignore_errors=True)

    for recipe, channel in mappings.items():
        if channel == args.channel:
            print(recipe, channel)
            recipe_path = Path("recipes").joinpath(recipe)
            recipe_content = yaml.safe_load(recipe_path.read_text())

            if recipe_content.get("build", {}).get("noarch"):
                platforms = [str(Platform.current())]
            else:
                platforms = ["win-64", "linux-64", "osx-arm64", "osx-64"]

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
                        recipe_path,
                    ],
                    check=True,
                )

    # Remove the build directory using shutil
    shutil.rmtree(channels_dir.joinpath("bld"), ignore_errors=True)


if __name__ == "__main__":
    main()
