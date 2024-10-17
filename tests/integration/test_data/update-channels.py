import subprocess
from pathlib import Path
import shutil
import tomllib


def main():
    platforms = ["win-64", "linux-64", "osx-arm64", "osx-64"]
    mappings = tomllib.loads(Path("mappings.toml").read_text())
    channels_dir = Path("channels")
    shutil.rmtree(channels_dir, ignore_errors=True)
    channels_dir.mkdir()

    for recipe, channel in mappings.items():
        print(recipe, channel)
        for platform in platforms:
            subprocess.run(
                [
                    "rattler-build",
                    "build",
                    "--target-platform",
                    platform,
                    "--output-dir",
                    f"channels/{channel}",
                    "--recipe",
                    f"recipes/{recipe}",
                ],
                check=True,
            )

        # Remove the build directory using shutil
        shutil.rmtree(Path(channel, "bld"), ignore_errors=True)


if __name__ == "__main__":
    main()
