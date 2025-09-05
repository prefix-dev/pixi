import argparse
from pathlib import Path
import shutil
import platform
import os

DEFAULT_DESTINATION_DIR = Path(os.getenv("PIXI_HOME", Path.home() / ".pixi")) / "bin"


def executable_extension(name: str) -> str:
    if platform.system() == "Windows":
        return name + ".exe"
    else:
        return name


def main() -> None:
    parser = argparse.ArgumentParser(
        description=f"Build pixi and copy the executable to {DEFAULT_DESTINATION_DIR} or a custom destination specified by --dest"
    )
    parser.add_argument("name", type=str, help="Name of the executable (e.g. pixid)")
    parser.add_argument(
        "--dest",
        type=Path,
        default=DEFAULT_DESTINATION_DIR,
        help=f"Destination directory for the executable, default: {DEFAULT_DESTINATION_DIR}",
    )
    parser.add_argument("--debug", action="store_true", help="Use the debug dir instead of release")

    print(os.environ["CARGO_TARGET_DIR"])
    args = parser.parse_args()

    rel_or_deb = "release" if not args.debug else "debug"

    built_executable_path = Path(os.environ["CARGO_TARGET_DIR"]).joinpath(
        rel_or_deb, executable_extension("pixi")
    )
    destination_path = args.dest.joinpath(executable_extension(args.name))

    print(f"Copying ({rel_or_deb}) the executable to {destination_path}")
    destination_path.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy(built_executable_path, destination_path)


if __name__ == "__main__":
    main()
