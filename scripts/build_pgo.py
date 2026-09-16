"""Build a Pixi release binary, using PGO when requested and runnable."""

import argparse
import os
import shlex
import shutil
import stat
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT: Path = Path(__file__).resolve().parent.parent
LINUX_MUSL_TARGET: str = "x86_64-unknown-linux-musl"
ZIG_TARGET: str = "x86_64-linux-musl"
# The Linux release targets are cross-compiled from an x86_64 glibc runner and
# link through Zig; every other target builds with its native toolchain.
ZIGBUILD_TARGETS: frozenset[str] = frozenset(
    {
        LINUX_MUSL_TARGET,
        "aarch64-unknown-linux-musl",
        "riscv64gc-unknown-linux-gnu",
    }
)
TRAINING_MANIFEST: Path = ROOT / "scripts" / "pgo" / "pixi.toml"


@dataclass(frozen=True)
class BuildOptions:
    target: str
    features: str
    no_default_features: bool
    auditable: bool


class Arguments(argparse.Namespace):
    target: str
    features: str
    no_default_features: bool
    auditable: bool
    pgo: bool
    output: Path | None


def cargo_args(options: BuildOptions) -> list[str]:
    arguments = [
        "--locked",
        "--release",
        "--manifest-path",
        str(ROOT / "crates" / "pixi" / "Cargo.toml"),
    ]
    if options.no_default_features:
        arguments.append("--no-default-features")
    if options.features:
        arguments.extend(["--features", options.features])
    arguments.extend(
        [
            "--bin",
            "pixi",
            "--target",
            options.target,
        ]
    )
    return arguments


def cargo_build_command(options: BuildOptions, *, zig_wrappers: bool) -> list[str]:
    """Cargo subcommand for a release build.

    `cargo-zigbuild` provides the cross toolchain, unless the caller already
    placed Zig wrappers for the compiler, archiver, and linker in the
    environment.
    """
    if options.auditable:
        return ["cargo", "auditable", "build"]
    if zig_wrappers or options.target not in ZIGBUILD_TARGETS:
        return ["cargo", "build"]
    return ["cargo", "zigbuild"]


def binary_path(target_dir: Path, target: str) -> Path:
    executable = "pixi.exe" if target.endswith("-pc-windows-msvc") else "pixi"
    return target_dir / target / "release" / executable


def can_train(target: str, host: str) -> bool:
    return target == host or (target == LINUX_MUSL_TARGET and host == "x86_64-unknown-linux-gnu")


def release_build(target_dir: Path, options: BuildOptions) -> Path:
    env: dict[str, str] = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target_dir)
    run([*cargo_build_command(options, zig_wrappers=False), *cargo_args(options)], env=env)
    return binary_path(target_dir, options.target)


def copy_output(binary: Path, output: Path | None) -> None:
    destination = output.resolve() if output else binary.resolve()
    if destination != binary.resolve():
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(binary, destination)
    print(destination)


def run(command: list[str], *, env: dict[str, str], quiet: bool = False) -> None:
    if quiet:
        result: subprocess.CompletedProcess[str] = subprocess.run(
            command,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )
        if result.returncode == 0:
            return
        sys.stdout.write(result.stdout)
        sys.stderr.write(result.stderr)
        raise subprocess.CalledProcessError(result.returncode, command)

    subprocess.run(command, env=env, check=True)


def rust_host() -> str:
    output: str = subprocess.check_output(["rustc", "-vV"], text=True)
    for line in output.splitlines():
        if line.startswith("host: "):
            return line.removeprefix("host: ")
    raise RuntimeError("rustc did not report its host target")


def llvm_profdata() -> Path:
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    executable_name = "llvm-profdata.exe" if os.name == "nt" else "llvm-profdata"
    executable = sysroot / "lib" / "rustlib" / rust_host() / "bin" / executable_name
    if not executable.is_file():
        raise FileNotFoundError(
            f"{executable} not found; install it with `rustup component add llvm-tools-preview`"
        )
    return executable


def encoded_rustflags(profile_flag: str, target: str) -> str:
    if encoded_flags := os.environ.get("CARGO_ENCODED_RUSTFLAGS"):
        flags: list[str] = [encoded_flags]
    else:
        flags = shlex.split(os.environ.get("RUSTFLAGS", ""))
    # CARGO_ENCODED_RUSTFLAGS overrides the target rustflags in .cargo/config.toml.
    if target.endswith("-pc-windows-msvc"):
        flags.append("-Ctarget-feature=+crt-static")
    flags.append(profile_flag)
    return "\x1f".join(flags)


def pgo_environment(target_dir: Path, profile_flag: str, target: str) -> dict[str, str]:
    env: dict[str, str] = os.environ.copy()
    env.pop("RUSTFLAGS", None)
    env.update(
        {
            "CARGO_INCREMENTAL": "0",
            "CARGO_TARGET_DIR": str(target_dir),
            "CARGO_ENCODED_RUSTFLAGS": encoded_rustflags(profile_flag, target),
        }
    )
    if target.endswith("-apple-darwin"):
        for variable in ("CFLAGS", "CXXFLAGS"):
            env[variable] = " ".join(
                filter(
                    None,
                    (
                        env.get(variable),
                        "-fno-profile-generate -fno-profile-use",
                    ),
                )
            )
    return env


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


def instrumented_build(
    target_dir: Path,
    profile_dir: Path,
    working_dir: Path,
    options: BuildOptions,
) -> Path:
    env = pgo_environment(
        target_dir,
        f"-Cprofile-generate={profile_dir}",
        options.target,
    )

    # Only the musl target trains, and its instrumented build needs the wrapper
    # that keeps the LLVM profile runtime alive.
    zig_wrappers = options.target == LINUX_MUSL_TARGET
    if zig_wrappers:
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

        target_env_name = options.target.replace("-", "_")
        env.update(
            {
                "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER": str(cc_wrapper),
                f"CC_{target_env_name}": str(cc_wrapper),
                f"CXX_{target_env_name}": str(cxx_wrapper),
                f"AR_{target_env_name}": str(ar_wrapper),
            }
        )

    run(
        [*cargo_build_command(options, zig_wrappers=zig_wrappers), *cargo_args(options)],
        env=env,
    )
    return binary_path(target_dir, options.target)


def train(binary: Path, profile_dir: Path, working_dir: Path) -> None:
    env: dict[str, str] = os.environ.copy()
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
    workloads: list[list[str]] = [
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


def optimized_build(
    target_dir: Path,
    profile_data: Path,
    options: BuildOptions,
) -> Path:
    env = pgo_environment(
        target_dir,
        f"-Cprofile-use={profile_data}",
        options.target,
    )
    run([*cargo_build_command(options, zig_wrappers=False), *cargo_args(options)], env=env)
    return binary_path(target_dir, options.target)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, help="Rust target triple")
    parser.add_argument(
        "--features",
        default="self_update,performance",
        help="Comma-separated Cargo features",
    )
    parser.add_argument(
        "--no-default-features",
        action="store_true",
        help="Disable Cargo's default features",
    )
    parser.add_argument(
        "--auditable",
        action="store_true",
        help="Build through cargo-auditable",
    )
    parser.add_argument(
        "--pgo",
        action="store_true",
        help="Use PGO when the target can run on this host",
    )
    parser.add_argument(
        "--output",
        type=Path,
        help="Copy the release binary to this path",
    )
    args = parser.parse_args(namespace=Arguments())
    options = BuildOptions(
        target=args.target,
        features=args.features,
        no_default_features=args.no_default_features,
        auditable=args.auditable,
    )

    host = rust_host()
    use_pgo = args.pgo and can_train(options.target, host)
    if args.pgo and not use_pgo:
        print(f"Target {options.target} cannot run on host {host}; building without PGO")
    if (
        options.target in ZIGBUILD_TARGETS
        and not options.auditable
        and shutil.which("cargo-zigbuild") is None
    ):
        raise FileNotFoundError(f"cargo-zigbuild is required to build {options.target}")
    if options.auditable and shutil.which("cargo-auditable") is None:
        raise FileNotFoundError("cargo-auditable is required")

    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    if not use_pgo:
        print("Building Pixi binary without PGO")
        copy_output(release_build(target_dir, options), args.output)
        return

    if not TRAINING_MANIFEST.is_file():
        raise FileNotFoundError(TRAINING_MANIFEST)
    if os.name == "nt":
        working_dir = Path(os.environ.get("RUNNER_TEMP", target_dir.anchor)) / "pixi-pgo"
    else:
        working_dir = target_dir / "pgo" / options.target
    instrumented_target_dir = working_dir / "i"
    optimized_target_dir = working_dir / "o"
    profile_dir = working_dir / "profiles"
    profile_data = working_dir / "pixi.profdata"

    shutil.rmtree(working_dir, ignore_errors=True)
    profile_dir.mkdir(parents=True)

    print("Building instrumented Pixi binary")
    instrumented_binary = instrumented_build(
        instrumented_target_dir,
        profile_dir,
        working_dir,
        options,
    )

    print("Training instrumented Pixi binary")
    train(instrumented_binary, profile_dir, working_dir)

    print("Merging PGO profiles")
    raw_profiles: list[Path] = list(profile_dir.glob("*.profraw"))
    if not raw_profiles:
        raise RuntimeError("training did not produce any PGO profiles")
    subprocess.run(
        [str(llvm_profdata()), "merge", "--output", str(profile_data), *raw_profiles],
        check=True,
    )

    print("Building PGO-optimized Pixi binary")
    optimized_binary = optimized_build(optimized_target_dir, profile_data, options)
    copy_output(optimized_binary, args.output or binary_path(target_dir, options.target))


if __name__ == "__main__":
    main()
