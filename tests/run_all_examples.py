import subprocess
import os
import argparse
from pathlib import Path
from typing import Tuple
from dataclasses import dataclass


@dataclass
class Results:
    succeeded: list[str]
    skipped: list[Tuple[str, str]]
    installed: list[str]
    failed: list[str]


def has_test_task(folder: Path, pixi_exec: Path) -> bool:
    command = [str(pixi_exec), "task", "--manifest-path", str(folder), "list"]
    result = subprocess.run(command, capture_output=True, text=True)
    return "test" in result.stderr


def run_test_in_subfolders(base_path: Path, pixi_exec: Path = Path("pixi")) -> Results:
    results = Results([], [], [], [])
    folders = [folder for folder in base_path.iterdir() if folder.is_dir()]

    for folder in folders:
        pixi_toml = folder / "pixi.toml"
        pyproject_toml = folder / "pyproject.toml"

        if pixi_toml.exists():
            manifest_path = pixi_toml
        elif pyproject_toml.exists():
            manifest_path = pyproject_toml
        else:
            continue

        do_install = False
        command = [str(pixi_exec), "run", "-v", "--manifest-path", str(manifest_path), "test"]
        if not has_test_task(manifest_path, pixi_exec):
            command = [str(pixi_exec), "-v", "install", "--manifest-path", str(manifest_path)]
            do_install = True

        print(f"Running command in {folder}: {' '.join(command)}")
        result = subprocess.run(command, capture_output=True, text=True)

        if result.returncode != 0:
            print(f"\033[91m ❌ {folder}\033[0m")
            print(f"\tOutput:\n{result.stdout.replace('\n', '\n\t')}")
            print(f"\tError:\n{result.stderr.replace('\n', '\n\t')}")
            results.failed.append(str(folder))
            continue

        if do_install:
            print(f"\033[93m 🚀 {folder} (just installed)\033[0m")
            results.installed.append(str(folder))
        else:
            print(f"\033[92m ✅ {folder}\033[0m")
            results.succeeded.append(str(folder))

    return results


def print_summary(results: Results, pixi_exec: Path):
    summary_text = f"║ ✅ {len(results.succeeded):<10} 🚀 {len(results.installed):<10} ❌ {len(results.failed):<10} 🤷 {len(results.skipped):<10} ║"

    # Calculate the actual length of the line, considering the emojis as single characters
    line_length = len(summary_text.encode("utf-8")) - 12

    summary_box_top = "╔" + "═" * line_length + "╗"
    summary_box_bottom = "╚" + "═" * line_length + "╝"
    summary_box_sep = "╟" + "─" * line_length + "╢"

    print("\n")
    print("✅ succeeded 🚀 installed ❌ failed 🤷 skipped")
    print(summary_box_top)
    print(f"║ Summary: {' ' * (line_length - len(' Summary: '))}║")
    print(summary_box_sep)
    print(summary_text)
    print(summary_box_bottom)

    if pixi_exec:
        print(f"Used custom binary at: {pixi_exec}")

    if results.skipped:
        print("\033[94mSkipped:\033[0m")
        for name, reason in results.skipped:
            print(f"\t - {name} ({reason})")

    if results.failed:
        print("\033[91mFailed:\033[0m")
        for name in results.failed:
            print(f"\t - {name}")


if __name__ == "__main__":
    try:
        parser = argparse.ArgumentParser(description="Run pixi commands in folders.")
        parser.add_argument(
            "--pixi-exec", type=str, required=False, help="Path to the pixi executable"
        )
        args = parser.parse_args()

        if args.pixi_exec:
            print(f"Using pixi binary at: {args.pixi_exec}")

        pixi_root = Path(os.environ.get("PIXI_PROJECT_ROOT", ""))
        pixi_exec = Path(args.pixi_exec) if args.pixi_exec else Path("pixi")
        results = run_test_in_subfolders(pixi_root / "examples", pixi_exec)

        print_summary(results, pixi_exec)

    except KeyboardInterrupt:
        pass
