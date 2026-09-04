"""Build the Linux x86-64 release binary with profile-guided optimization."""

import os
import shlex
import shutil
import stat
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TARGET = "x86_64-unknown-linux-musl"
ZIG_TARGET = "x86_64-linux-musl"
TRAINING_MANIFEST = ROOT / "scripts" / "pgo" / "pixi.toml"

CARGO_ARGS = [
    "--locked",
    "--release",
    "--manifest-path",
    str(ROOT / "crates" / "pixi" / "Cargo.toml"),
    "--features",
    "self_update,performance",
    "--bin",
    "pixi",
    "--target",
    TARGET,
]


def run(command: list[str], *, env: dict[str, str], quiet: bool = False) -> None:
    if quiet:
        result = subprocess.run(command, env=env, text=True, capture_output=True)
        if result.returncode == 0:
            return
        sys.stdout.write(result.stdout)
        sys.stderr.write(result.stderr)
        raise subprocess.CalledProcessError(result.returncode, command)

    subprocess.run(command, env=env, check=True)


def rust_host() -> str:
    output = subprocess.check_output(["rustc", "-vV"], text=True)
    for line in output.splitlines():
        if line.startswith("host: "):
            return line.removeprefix("host: ")
    raise RuntimeError("rustc did not report its host target")


def llvm_profdata() -> Path:
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    executable = sysroot / "lib" / "rustlib" / rust_host() / "bin" / "llvm-profdata"
    if not executable.is_file():
        raise FileNotFoundError(
            f"{executable} not found; install it with `rustup component add llvm-tools-preview`"
        )
    return executable


def encoded_rustflags(profile_flag: str) -> str:
    if encoded_flags := os.environ.get("CARGO_ENCODED_RUSTFLAGS"):
        return f"{encoded_flags}\x1f{profile_flag}"
    return "\x1f".join([*shlex.split(os.environ.get("RUSTFLAGS", "")), profile_flag])


def write_zig_wrapper(path: Path, compiler: str) -> None:
    # Zig 0.16 treats rustc's `-u __llvm_profile_runtime` as an input file.
    # Passing the same option directly to the linker keeps the PGO runtime alive.
    path.write_text(
        f"""#!/usr/bin/env bash
set -euo pipefail
rewritten=()
while (($#)); do
  if [[ "$1" == "-u" && $# -ge 2 ]]; then
    rewritten+=("-Wl,-u,$2")
    shift 2
  else
    rewritten+=("$1")
    shift
  fi
done
exec cargo-zigbuild zig {compiler} -- -g -fno-sanitize=all -target {ZIG_TARGET} "${{rewritten[@]}}"
"""
    )
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def instrumented_build(target_dir: Path, profile_dir: Path, working_dir: Path) -> Path:
    wrappers = working_dir / "wrappers"
    wrappers.mkdir(parents=True)
    cc_wrapper = wrappers / "zigcc"
    cxx_wrapper = wrappers / "zigcxx"
    ar_wrapper = wrappers / "zigar"
    write_zig_wrapper(cc_wrapper, "cc")
    write_zig_wrapper(cxx_wrapper, "c++")
    ar_wrapper.write_text(
        '#!/usr/bin/env bash\nset -euo pipefail\nexec cargo-zigbuild zig ar -- "$@"\n'
    )
    ar_wrapper.chmod(ar_wrapper.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    target_env_name = TARGET.replace("-", "_")
    env = os.environ.copy()
    env.pop("RUSTFLAGS", None)
    env.update(
        {
            "CARGO_INCREMENTAL": "0",
            "CARGO_TARGET_DIR": str(target_dir),
            "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER": str(cc_wrapper),
            f"CC_{target_env_name}": str(cc_wrapper),
            f"CXX_{target_env_name}": str(cxx_wrapper),
            f"AR_{target_env_name}": str(ar_wrapper),
            "CARGO_ENCODED_RUSTFLAGS": encoded_rustflags(f"-Cprofile-generate={profile_dir}"),
        }
    )
    run(["cargo", "build", *CARGO_ARGS], env=env)
    return target_dir / TARGET / "release" / "pixi"


def train(binary: Path, profile_dir: Path, working_dir: Path) -> None:
    env = os.environ.copy()
    env.update(
        {
            "LLVM_PROFILE_FILE": str(profile_dir / "pixi-%m-%p.profraw"),
            "PIXI_CACHE_DIR": str(working_dir / "cache"),
            "PIXI_HOME": str(working_dir / "home"),
            "PIXI_NO_CONFIG": "true",
            "PIXI_NO_PROGRESS": "true",
            "PIXI_COLOR": "never",
        }
    )
    manifest = str(TRAINING_MANIFEST)
    workloads = [
        ["--version"],
        ["--help"],
        ["task", "list", "-m", manifest],
        ["workspace", "environment", "list", "-m", manifest],
        ["lock", "--dry-run", "-m", manifest],
        ["shell-hook", "--as-is", "-m", manifest],
        ["global", "list"],
    ]
    for arguments in workloads:
        run([str(binary), *arguments], env=env, quiet=True)


def optimized_build(target_dir: Path, profile_data: Path) -> Path:
    env = os.environ.copy()
    env.pop("RUSTFLAGS", None)
    env.update(
        {
            "CARGO_INCREMENTAL": "0",
            "CARGO_TARGET_DIR": str(target_dir),
            "CARGO_ENCODED_RUSTFLAGS": encoded_rustflags(f"-Cprofile-use={profile_data}"),
        }
    )
    run(["cargo", "zigbuild", *CARGO_ARGS], env=env)
    return target_dir / TARGET / "release" / "pixi"


def main() -> None:
    if shutil.which("cargo-zigbuild") is None:
        raise FileNotFoundError("cargo-zigbuild is required")
    if not TRAINING_MANIFEST.is_file():
        raise FileNotFoundError(TRAINING_MANIFEST)

    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    working_dir = target_dir / "pgo"
    instrumented_target_dir = working_dir / "instrumented"
    optimized_target_dir = working_dir / "optimized"
    profile_dir = working_dir / "profiles"
    profile_data = working_dir / "pixi.profdata"

    shutil.rmtree(working_dir, ignore_errors=True)
    profile_dir.mkdir(parents=True)

    print("Building instrumented Pixi binary")
    instrumented_binary = instrumented_build(instrumented_target_dir, profile_dir, working_dir)

    print("Training instrumented Pixi binary")
    train(instrumented_binary, profile_dir, working_dir)

    print("Merging PGO profiles")
    raw_profiles = list(profile_dir.glob("*.profraw"))
    if not raw_profiles:
        raise RuntimeError("training did not produce any PGO profiles")
    subprocess.run(
        [str(llvm_profdata()), "merge", "--output", str(profile_data), *raw_profiles],
        check=True,
    )

    print("Building PGO-optimized Pixi binary")
    optimized_binary = optimized_build(optimized_target_dir, profile_data)
    binary = target_dir / TARGET / "release" / "pixi"
    binary.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(optimized_binary, binary)
    print(binary)


if __name__ == "__main__":
    main()
