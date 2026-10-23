#!/usr/bin/env python3
"""Run paired performance samples for two builds on the same runner."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import tempfile


SUITE = Path(__file__).with_name("performance-regression.py")
METRICS = {
    "install_cold_ms": "install cold median (ms)",
    "install_warm_ms": "install warm median (ms)",
    "metadata_parser_ms": "metadata parser median (ms)",
    "update_commit_ms": "update commit median (ms)",
    "update_rollback_ms": "health-failure rollback median (ms)",
}
HARNESS_FILES = (
    "crates/ccl-core/examples/p7_benchmark.rs",
    "crates/ccl-cli/tests/update_process.rs",
)


def commit_for(repo):
    return subprocess.run(
        ["git", "-C", str(repo), "rev-parse", "--verify", "HEAD^{commit}"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def sample(build, repo, scratch, pair, arm, iterations, commit):
    report_path = scratch / f"pair-{pair:02}-{arm}.json"
    command = [
        sys.executable,
        str(SUITE),
        "--core-example",
        str(build["core"].resolve()),
        "--cli-binary",
        str(build["cli"].resolve()),
        "--repo",
        str(repo.resolve()),
        "--iterations",
        str(iterations),
        "--run-update-benchmark",
        "--output",
        str(report_path),
    ]
    run_env = os.environ.copy()
    run_env.update({
        "CCL_BUILD_CHANNEL": "dev",
        "CCL_BUILD_COMMIT": commit,
    })
    subprocess.run(command, check=True, env=run_env)
    report = json.loads(report_path.read_text(encoding="utf-8"))
    observation = report.get("observation")
    metrics = observation.get("metrics") if isinstance(observation, dict) else None
    if not isinstance(metrics, dict):
        raise RuntimeError(f"pair {pair + 1} {arm}: report omitted observation metrics")
    for key, label in METRICS.items():
        value = metrics.get(key)
        if (isinstance(value, bool) or not isinstance(value, (int, float))
                or not math.isfinite(value) or value <= 0):
            raise RuntimeError(
                f"pair {pair + 1} {arm}: {label} must be finite and positive; got {value!r}"
            )
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-core-example", required=True, type=Path)
    parser.add_argument("--baseline-cli-binary", required=True, type=Path)
    parser.add_argument("--baseline-repo", required=True, type=Path)
    parser.add_argument("--candidate-core-example", required=True, type=Path)
    parser.add_argument("--candidate-cli-binary", required=True, type=Path)
    parser.add_argument("--candidate-repo", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--pairs", type=int, default=5)
    parser.add_argument("--iterations-per-run", type=int, default=3)
    parser.add_argument("--max-regression-percent", type=float, default=25)
    args = parser.parse_args()
    if args.pairs < 3 or args.iterations_per_run < 3:
        raise SystemExit("use at least three pairs and three iterations per build sample")

    builds = {
        "baseline": {
            "core": args.baseline_core_example,
            "cli": args.baseline_cli_binary,
            "repo": args.baseline_repo,
        },
        "candidate": {
            "core": args.candidate_core_example,
            "cli": args.candidate_cli_binary,
            "repo": args.candidate_repo,
        },
    }
    for build in builds.values():
        for key in ("core", "cli", "repo"):
            if not build[key].exists():
                raise SystemExit(f"missing {key} path: {build[key]}")
    commits = {name: commit_for(build["repo"]) for name, build in builds.items()}
    harness_hashes = {
        path: {
            name: sha256(build["repo"] / path)
            for name, build in builds.items()
        }
        for path in HARNESS_FILES
    }
    if any(len(set(hashes.values())) != 1 for hashes in harness_hashes.values()):
        raise SystemExit("baseline and candidate benchmark harness hashes differ")

    pair_results = []
    with tempfile.TemporaryDirectory(prefix="ccl-p7-paired-") as temp_name:
        scratch = Path(temp_name)
        for pair in range(args.pairs):
            order = ("baseline", "candidate") if pair % 2 == 0 else ("candidate", "baseline")
            measurements = {}
            for arm in order:
                build = builds[arm]
                measurements[arm] = sample(
                    build, build["repo"], scratch, pair, arm, args.iterations_per_run,
                    commits[arm],
                )
            deltas = {}
            metrics_by_arm = {
                arm: measurements[arm]["observation"]["metrics"]
                for arm in order
            }
            for key, label in METRICS.items():
                before = metrics_by_arm["baseline"][key]
                after = metrics_by_arm["candidate"][key]
                deltas[key] = {
                    "label": label,
                    "baseline": before,
                    "candidate": after,
                    "change_percent": round((after / before - 1) * 100, 2),
                }
            pair_results.append({
                "pair": pair + 1,
                "order": list(order),
                "metrics": deltas,
                "reports": measurements,
            })

    comparisons = []
    failed = []
    for key, label in METRICS.items():
        changes = [
            result["metrics"][key]["change_percent"]
            for result in pair_results
            if key in result["metrics"]
        ]
        if not changes:
            continue
        change = statistics.median(changes)
        comparisons.append({
            "metric": key,
            "label": label,
            "paired_change_percent": round(change, 2),
            "pair_changes_percent": changes,
            "pairs": len(changes),
            "limit_percent": args.max_regression_percent,
        })
        if change > args.max_regression_percent:
            failed.append(key)

    report = {
        "schema_version": 1,
        "benchmark_suite": "p7_paired_same_runner",
        "measurement_harness_version": 2,
        "observed_at": datetime.now(timezone.utc).isoformat(),
        "runner": {
            "os": os.environ.get("RUNNER_OS", platform.system()),
            "arch": os.environ.get("RUNNER_ARCH", platform.machine()),
            "image_os": os.environ.get("ImageOS"),
            "image_version": os.environ.get("ImageVersion"),
            "platform": platform.platform(),
            "processor": platform.processor(),
            "logical_cpus": os.cpu_count(),
        },
        "toolchain": subprocess.run(
            ["rustc", "--version"], check=True, capture_output=True, text=True
        ).stdout.strip(),
        "commits": commits,
        "benchmark_harness_sha256": harness_hashes,
        "pairs": args.pairs,
        "iterations_per_build_sample": args.iterations_per_run,
        "comparisons": comparisons,
        "pair_results": pair_results,
        "gate": {"state": "regression" if failed else "passed", "failed_metrics": failed},
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report["gate"], sort_keys=True))
    for comparison in comparisons:
        print(
            f"{comparison['label']}: {comparison['paired_change_percent']:+.2f}% "
            f"({comparison['pairs']} paired observations)"
        )
    if failed:
        raise SystemExit(f"paired performance regression exceeds limit: {failed}")


if __name__ == "__main__":
    main()
