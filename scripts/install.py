import argparse
from pathlib import Path
import shutil
import platform
import os


def executable_extension(name: str) -> str:
    if platform.system() == "Windows":
        return name + ".exe"
    else:
        return name


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Build pixi and copy the executable to ~/.pixi/bin/"
    )
    parser.add_argument("name", type=str, help="Name of the executable (e.g. pixid)")

    args = parser.parse_args()

    built_executable_path = Path(os.environ["CARGO_TARGET_DIR"]).joinpath(
        "release", executable_extension("pixi")
    )
    destination_path = Path.home().joinpath(".pixi", "bin", executable_extension(args.name))

    print(f"Copying the executable to {destination_path}")
    shutil.copy(built_executable_path, destination_path)

    print("Done!")


if __name__ == "__main__":
    main()
