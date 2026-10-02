"""Determine per-target build settings and write them to GITHUB_ENV.

The pixi binary is built with the same feature set on every target
(self_update + performance); the per-target tweaks are the jemalloc page size
for 64 KiB-page aarch64 Linux (see
https://github.com/prefix-dev/pixi/issues/2936) and the statically linked CRT
on Windows.

Usage:
    pixi run -e release build-options --target aarch64-unknown-linux-musl
"""

import argparse
import os
from pathlib import Path

# Cargo picks extra rustc flags from exactly one source: if RUSTFLAGS is set it
# wins outright and every `[target.*].rustflags` entry in .cargo/config.toml is
# ignored. The release workflow needs RUSTFLAGS for `-D warnings`, so any flag
# that config.toml would otherwise contribute has to be repeated here.
BASE_RUSTFLAGS = ["-D", "warnings"]

# Statically link the MSVC CRT. Without this the binary imports
# vcruntime140.dll, which is not present in stock Windows Server container
# images, and pixi.exe fails to start with STATUS_DLL_NOT_FOUND.
# See https://github.com/prefix-dev/pixi/issues/6915.
MSVC_RUSTFLAGS = ["-C", "target-feature=+crt-static"]


def rustflags_for(target: str) -> list[str]:
    flags = list(BASE_RUSTFLAGS)
    if target.endswith("-pc-windows-msvc"):
        flags += MSVC_RUSTFLAGS
    return flags


def build_env(target: str) -> dict[str, str]:
    env: dict[str, str] = {"RUSTFLAGS": " ".join(rustflags_for(target))}
    # aarch64 Linux runners may use 64 KiB pages; jemalloc must be told the
    # page size at compile time so the binary runs on those hosts.
    if target.startswith("aarch64-") and "linux" in target:
        env["JEMALLOC_SYS_WITH_LG_PAGE"] = "16"
    return env


def main() -> None:
    parser = argparse.ArgumentParser(description="Determine build options for a target")
    parser.add_argument("--target", required=True, help="Rust target triple")
    args = parser.parse_args()

    env = build_env(args.target)

    for key, value in env.items():
        print(f"{key}={value}")

    github_env = os.environ.get("GITHUB_ENV")
    if github_env:
        with Path(github_env).open("a") as f:
            for key, value in env.items():
                f.write(f"{key}={value}\n")


if __name__ == "__main__":
    main()
