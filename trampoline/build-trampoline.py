import subprocess
from pathlib import Path
import argparse


def get_default_target() -> str:
    result = subprocess.run(["rustc", "-vV"], capture_output=True, text=True, check=True)
    for line in result.stdout.splitlines():
        if line.startswith("host:"):
            return line.split(":")[1].strip()
    raise RuntimeError("Unable to determine default target")


def build_trampoline_binary(target: str, target_dir: Path) -> None:
    _ = subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "--target",
            target,
            "--target-dir",
            target_dir,
            "--manifest-path",
            "trampoline/Cargo.toml",
        ],
        check=True,
    )


def compress_binary(target: str, target_dir: Path) -> None:
    is_windows = target.endswith("windows-msvc")
    trampolines_dir = Path("trampoline", "binaries")
    trampolines_dir.mkdir(parents=True, exist_ok=True)

    extension = ".exe" if is_windows else ""
    binary_path = target_dir.joinpath(target, "release", f"pixi_trampoline{extension}")
    compressed_path = trampolines_dir.joinpath(f"pixi-trampoline-{target}{extension}.zst")

    _ = subprocess.run(["zstd", binary_path, "-o", compressed_path, "--force"], check=True)


def main(target: str) -> None:
    target_dir = Path("target/trampoline")
    build_trampoline_binary(target, target_dir)
    compress_binary(target, target_dir)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Build and compress trampoline binaries.")
    _ = parser.add_argument(
        "--target",
        type=str,
        help="The target triple for the build (e.g., x86_64-unknown-linux-musl).",
    )
    args = parser.parse_args()
    target = args.target if args.target else get_default_target()
    main(target)
