"""Compare baseline and PGO Pixi binaries with warmed, isolated workspaces."""

import argparse
import json
import os
import shutil
import statistics
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import TypedDict

ROOT = Path(__file__).resolve().parent.parent
TRAINING_MANIFEST = ROOT / "scripts" / "pgo" / "pixi.toml"
HELD_OUT_MANIFEST = ROOT / "scripts" / "pgo" / "benchmark.toml"


@dataclass(frozen=True)
class Workload:
    name: str
    arguments: tuple[str, ...]
    held_out: bool = False


class TimingResult(TypedDict):
    baseline_ms: float
    pgo_ms: float
    change_percent: float


class WorkloadResult(TimingResult):
    name: str
    held_out: bool


class BinarySizeResult(TypedDict):
    baseline_bytes: int
    pgo_bytes: int
    change_percent: float


class Results(TypedDict):
    rounds: int
    workloads: list[WorkloadResult]
    training_suite: TimingResult
    binary_size: BinarySizeResult


def benchmark_environment(root: Path) -> dict[str, str]:
    environment = os.environ.copy()
    environment.update(
        {
            "PIXI_CACHE_DIR": str(root / "cache"),
            "PIXI_HOME": str(root / "home"),
            "PIXI_NO_CONFIG": "true",
            "PIXI_NO_PROGRESS": "true",
            "PIXI_COLOR": "never",
        }
    )
    return environment


def run_once(binary: Path, workload: Workload, environment: dict[str, str]) -> float:
    started = time.perf_counter_ns()
    result = subprocess.run(
        [str(binary), *workload.arguments],
        env=environment,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=300,
    )
    elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
    if result.returncode != 0:
        diagnostic = subprocess.run(
            [str(binary), *workload.arguments],
            env=environment,
            text=True,
            capture_output=True,
            timeout=300,
        )
        raise RuntimeError(
            f"{workload.name} failed with exit code {result.returncode}:\n"
            f"{diagnostic.stdout}{diagnostic.stderr}"
        )
    return elapsed_ms


def change_percent(baseline: float, pgo: float) -> float:
    return ((pgo / baseline) - 1) * 100


def format_summary(results: Results) -> str:
    lines = [
        "## PGO comparison",
        "",
        "Negative deltas are improvements.",
        "",
        "| Workload | Baseline | PGO | Delta |",
        "| --- | ---: | ---: | ---: |",
    ]
    for workload in results["workloads"]:
        lines.append(
            f"| {workload['name']} | {workload['baseline_ms']:.1f} ms | "
            f"{workload['pgo_ms']:.1f} ms | {workload['change_percent']:+.1f}% |"
        )

    suite = results["training_suite"]
    size = results["binary_size"]
    lines.extend(
        [
            f"| **Training suite** | **{suite['baseline_ms']:.1f} ms** | "
            f"**{suite['pgo_ms']:.1f} ms** | **{suite['change_percent']:+.1f}%** |",
            f"| Binary size | {size['baseline_bytes'] / 1_048_576:.1f} MiB | "
            f"{size['pgo_bytes'] / 1_048_576:.1f} MiB | {size['change_percent']:+.1f}% |",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--pgo", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--rounds", type=int, default=7)
    args = parser.parse_args()

    baseline = args.baseline.resolve()
    pgo = args.pgo.resolve()
    for binary in (baseline, pgo):
        if not binary.is_file():
            raise FileNotFoundError(binary)
    if args.rounds < 1:
        parser.error("--rounds must be at least 1")

    with tempfile.TemporaryDirectory(prefix="pixi-pgo-benchmark-") as temporary_directory:
        temporary_root = Path(temporary_directory)
        variants: dict[str, tuple[Path, dict[str, str]]] = {}
        workloads_by_variant: dict[str, list[Workload]] = {}
        for name, binary in (("baseline", baseline), ("pgo", pgo)):
            variant_root = temporary_root / name
            variant_root.mkdir()
            training_manifest = variant_root / "training" / "pixi.toml"
            held_out_manifest = variant_root / "held-out" / "pixi.toml"
            training_manifest.parent.mkdir()
            held_out_manifest.parent.mkdir()
            shutil.copy2(TRAINING_MANIFEST, training_manifest)
            shutil.copy2(HELD_OUT_MANIFEST, held_out_manifest)
            variants[name] = (binary, benchmark_environment(variant_root))
            workloads_by_variant[name] = [
                Workload("version", ("--version",)),
                Workload("help", ("--help",)),
                Workload("task list", ("task", "list", "-m", str(training_manifest))),
                Workload(
                    "environment list",
                    ("workspace", "environment", "list", "-m", str(training_manifest)),
                ),
                Workload("training solve", ("lock", "--dry-run", "-m", str(training_manifest))),
                Workload("shell hook", ("shell-hook", "--as-is", "-m", str(training_manifest))),
                Workload("global list", ("global", "list")),
                Workload(
                    "held-out solve",
                    ("lock", "--dry-run", "-m", str(held_out_manifest)),
                    held_out=True,
                ),
            ]

        samples: dict[str, dict[str, list[float]]] = {
            workload.name: {"baseline": [], "pgo": []}
            for workload in workloads_by_variant["baseline"]
        }
        for workload_index, baseline_workload in enumerate(workloads_by_variant["baseline"]):
            pgo_workload = workloads_by_variant["pgo"][workload_index]
            for variant_name, workload in (("baseline", baseline_workload), ("pgo", pgo_workload)):
                binary, environment = variants[variant_name]
                run_once(binary, workload, environment)

            for round_index in range(args.rounds):
                order = ("baseline", "pgo") if round_index % 2 == 0 else ("pgo", "baseline")
                for variant_name in order:
                    workload = workloads_by_variant[variant_name][workload_index]
                    binary, environment = variants[variant_name]
                    samples[workload.name][variant_name].append(
                        run_once(binary, workload, environment)
                    )

    workload_results: list[WorkloadResult] = []
    for workload in workloads_by_variant["baseline"]:
        baseline_ms = statistics.median(samples[workload.name]["baseline"])
        pgo_ms = statistics.median(samples[workload.name]["pgo"])
        workload_results.append(
            {
                "name": workload.name,
                "held_out": workload.held_out,
                "baseline_ms": baseline_ms,
                "pgo_ms": pgo_ms,
                "change_percent": change_percent(baseline_ms, pgo_ms),
            }
        )

    training_results = [result for result in workload_results if not result["held_out"]]
    training_baseline_ms = sum(result["baseline_ms"] for result in training_results)
    training_pgo_ms = sum(result["pgo_ms"] for result in training_results)
    baseline_size = baseline.stat().st_size
    pgo_size = pgo.stat().st_size
    results: Results = {
        "rounds": args.rounds,
        "workloads": workload_results,
        "training_suite": {
            "baseline_ms": training_baseline_ms,
            "pgo_ms": training_pgo_ms,
            "change_percent": change_percent(training_baseline_ms, training_pgo_ms),
        },
        "binary_size": {
            "baseline_bytes": baseline_size,
            "pgo_bytes": pgo_size,
            "change_percent": change_percent(baseline_size, pgo_size),
        },
    }

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
    summary = format_summary(results)
    print(summary)
    if github_summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(github_summary).open("a", encoding="utf-8") as summary_file:
            summary_file.write(summary)


if __name__ == "__main__":
    main()
