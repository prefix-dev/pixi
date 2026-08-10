"""Determine per-target build settings and write them to GITHUB_ENV.

The pixi binary is built with the same feature set on every target
(self_update + performance); the only per-target tweak is the jemalloc page
size for 64 KiB-page aarch64 Linux, see
https://github.com/prefix-dev/pixi/issues/2936.

Usage:
    pixi run -e release build-options --target aarch64-unknown-linux-musl
"""

import argparse
import os
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description="Determine build options for a target")
    parser.add_argument("--target", required=True, help="Rust target triple")
    args = parser.parse_args()

    target: str = args.target

    env: dict[str, str] = {}
    # aarch64 Linux runners may use 64 KiB pages; jemalloc must be told the
    # page size at compile time so the binary runs on those hosts.
    if target.startswith("aarch64-") and "linux" in target:
        env["JEMALLOC_SYS_WITH_LG_PAGE"] = "16"

    for key, value in env.items():
        print(f"{key}={value}")

    github_env = os.environ.get("GITHUB_ENV")
    if github_env:
        with Path(github_env).open("a") as f:
            for key, value in env.items():
                f.write(f"{key}={value}\n")


if __name__ == "__main__":
    main()
