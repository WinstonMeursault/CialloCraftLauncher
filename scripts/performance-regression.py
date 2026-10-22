#!/usr/bin/env python3
"""Run deterministic install, parser and updater performance samples."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
import platform
from pathlib import Path
import statistics
import subprocess
import tempfile
import time
import zipfile


FIXTURE_MANIFEST = {
    "formatVersion": 1,
    "game": "minecraft",
    "versionId": "1.21.1",
    "name": "CCL deterministic parser fixture",
    "files": [],
    "dependencies": {"minecraft": "1.21.1", "fabric-loader": "0.16.14"},
}
UPDATE_TEST = [
    "cargo", "test", "--release", "--locked", "-p", "ccl-cli", "--test", "update_process",
    "benchmark_update_transactions", "--", "--ignored", "--nocapture",
]


def median(values):
    return round(statistics.median(values), 3)


def run_install_samples(binary: Path, iterations: int, scratch: Path):
    reports = []
    for index in range(iterations):
        output = scratch / f"install-{index}.json"
        result = subprocess.run(
            [str(binary), "--output", str(output)], capture_output=True, text=True, check=False
        )
        if result.returncode:
            raise RuntimeError(f"install benchmark failed:\n{result.stdout}\n{result.stderr}")
        report = json.loads(output.read_text(encoding="utf-8"))
        if report.get("benchmark") != "vanilla_install_local_mirror_cold_warm":
            raise RuntimeError("unexpected core install benchmark identity")
        if not report.get("fixture_hashes_verified"):
            raise RuntimeError("benchmark fixture hash verification was not confirmed")
        if report["cold"]["downloaded_files"] < 4 or report["warm"]["reused_files"] < 4:
            raise RuntimeError("benchmark did not exercise cold downloads and warm cache reuse")
        if report["warm"]["downloaded_files"] != 0:
            raise RuntimeError("warm pass unexpectedly downloaded artifact bytes")
        reports.append(report)
        print(result.stdout.strip())

    fixture_hashes = {report["fixture_sha256"] for report in reports}
    if len(fixture_hashes) != 1:
        raise RuntimeError("install fixture changed between repetitions")
    return {
        "benchmark": reports[0]["benchmark"],
        "fixture": reports[0]["fixture"],
        "fixture_sha256": reports[0]["fixture_sha256"],
        "iterations": iterations,
        "cold_elapsed_ms_samples": [round(r["cold"]["elapsed_ms"], 3) for r in reports],
        "cold_elapsed_ms_median": median([r["cold"]["elapsed_ms"] for r in reports]),
        "warm_elapsed_ms_samples": [round(r["warm"]["elapsed_ms"], 3) for r in reports],
        "warm_elapsed_ms_median": median([r["warm"]["elapsed_ms"] for r in reports]),
        "cold_transferred_bytes": reports[0]["cold"]["transferred_bytes"],
        "warm_transferred_bytes": reports[0]["warm"]["transferred_bytes"],
        "cold_downloaded_files": reports[0]["cold"]["downloaded_files"],
        "warm_reused_files": reports[0]["warm"]["reused_files"],
        "logical_disk_bytes": reports[0]["final_disk"]["logical_bytes"],
        "disk_files": reports[0]["final_disk"]["files"],
        "peak_memory": reports[0]["peak_memory"],
        "http_request_samples": [
            {"cold": r["cold"]["http_requests"], "warm": r["warm"]["http_requests"]}
            for r in reports
        ],
    }


def run_parser_samples(binary: Path, iterations: int, scratch: Path):
    fixture = scratch / "fixed-fixture.mrpack"
    manifest_bytes = json.dumps(FIXTURE_MANIFEST, sort_keys=True, separators=(",", ":")).encode()
    with zipfile.ZipFile(fixture, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        info = zipfile.ZipInfo("modrinth.index.json", date_time=(2020, 1, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        archive.writestr(info, manifest_bytes, compresslevel=6)
    fixture_hash = hashlib.sha256(fixture.read_bytes()).hexdigest()
    samples = []
    for _ in range(iterations):
        started = time.perf_counter_ns()
        result = subprocess.run(
            [str(binary), "--json", "pack", "inspect", str(fixture)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            check=False,
        )
        samples.append((time.perf_counter_ns() - started) / 1_000_000)
        if result.returncode:
            raise RuntimeError(result.stderr.decode("utf-8", errors="replace"))
    return {
        "benchmark": "modrinth_manifest_inspect_process_cold_start",
        "fixture": "fixed minimal Modrinth v1 manifest in deterministic ZIP",
        "fixture_sha256": fixture_hash,
        "iterations": iterations,
        "samples_ms": [round(value, 3) for value in samples],
        "median_ms": median(samples),
        "mean_ms": round(statistics.mean(samples), 3),
        "coverage": ["CLI process startup", "ZIP open/decompression", "manifest JSON parsing", "manifest validation"],
    }


def run_update_benchmark(cwd: Path):
    result = subprocess.run(UPDATE_TEST, cwd=cwd, capture_output=True, text=True, check=False)
    combined = result.stdout + "\n" + result.stderr
    if result.returncode:
        raise RuntimeError(f"update transaction benchmark failed:\n{combined}")
    for line in combined.splitlines():
        prefix = "CCL_UPDATE_BENCHMARK="
        if line.startswith(prefix):
            data = json.loads(line[len(prefix):])
            if not isinstance(data, dict):
                raise RuntimeError("update benchmark returned malformed or empty metrics")
            commit_samples = data.get("commit_ms")
            rollback_samples = data.get("rollback_ms")
            if not (isinstance(commit_samples, list) and commit_samples and
                    isinstance(rollback_samples, list) and rollback_samples):
                raise RuntimeError("update benchmark omitted commit/rollback timing samples")
            return {
                "benchmark": data.get("scenario", "helper_update_and_rollback"),
                "commit_samples_ms": commit_samples,
                "commit_median_ms": median(commit_samples),
                "rollback_samples_ms": rollback_samples,
                "rollback_median_ms": median(rollback_samples),
                "candidate_bytes": data.get("candidate_bytes"),
                "samples_per_scenario": data.get("samples_per_scenario"),
                "verified": True,
                "verification": "helper returned expected commit/rollback result and journal state for every sample",
            }
    return None


def compare_baseline(report, baseline, limit):
    current = report["observation"]
    observations = []
    baseline_path = Path(baseline)
    files = [baseline_path] if baseline_path.is_file() else list(baseline_path.rglob("report.json"))
    seen = {current["id"]}
    for path in files:
        try:
            candidate = json.loads(path.read_text(encoding="utf-8"))
            observation = candidate.get("observation")
        except (OSError, json.JSONDecodeError):
            continue
        if not isinstance(observation, dict) or not compatible_observations(current, observation):
            continue
        identifier = observation.get("id")
        if identifier and identifier not in seen:
            seen.add(identifier)
            observations.append(observation)
    observations.sort(key=lambda item: item.get("observed_at", ""))
    prior = observations[-5:]
    history = prior[-4:] + [current]
    report["baseline_history"] = history[-5:]

    comparisons = []
    failed = []
    baseline_gate = {
        "minimum_observations": 3,
        "compatible_observations": len(prior),
        "history_limit": 5,
        "regression_limit_percent": limit,
    }
    if len(prior) < 3:
        baseline_gate["state"] = "collecting"
        report["baseline_comparison"] = comparisons
        report["baseline_gate"] = baseline_gate
        return False

    metrics = [
        ("install.cold_elapsed_ms_median", current["metrics"].get("install_cold_ms"), "install_cold_ms"),
        ("install.warm_elapsed_ms_median", current["metrics"].get("install_warm_ms"), "install_warm_ms"),
        ("metadata_parser.median_ms", current["metrics"].get("metadata_parser_ms"), "metadata_parser_ms"),
        ("update_transaction.commit_median_ms", current["metrics"].get("update_commit_ms"), "update_commit_ms"),
        ("update_transaction.rollback_median_ms", current["metrics"].get("update_rollback_ms"), "update_rollback_ms"),
    ]
    for name, value, key in metrics:
        prior_values = [item["metrics"].get(key) for item in prior]
        prior_values = [float(item) for item in prior_values if isinstance(item, (int, float)) and item > 0]
        if value is None or len(prior_values) < 3:
            continue
        reference = statistics.median(prior_values[-5:])
        change = (float(value) / reference - 1) * 100 if reference else 0
        comparisons.append({
            "metric": name,
            "current": value,
            "baseline_median": round(reference, 3),
            "baseline_observations": len(prior_values),
            "change_percent": round(change, 2),
            "limit_percent": limit,
        })
        if change > limit:
            failed.append(name)
    baseline_gate["state"] = "regression" if failed else "enforced"
    baseline_gate["failed_metrics"] = failed
    report["baseline_comparison"] = comparisons
    report["baseline_gate"] = baseline_gate
    return bool(failed)


def compatible_observations(current, candidate):
    return (
        candidate.get("benchmark_suite") == current.get("benchmark_suite")
        and candidate.get("runner_os") == current.get("runner_os")
        and candidate.get("runner_arch") == current.get("runner_arch")
        and candidate.get("toolchain") == current.get("toolchain")
        and candidate.get("build_profile") == current.get("build_profile")
        and candidate.get("fixtures", {}).get("install_sha256") == current.get("fixtures", {}).get("install_sha256")
        and candidate.get("fixtures", {}).get("metadata_parser_sha256") == current.get("fixtures", {}).get("metadata_parser_sha256")
        and candidate.get("fixtures", {}).get("update_candidate_bytes") == current.get("fixtures", {}).get("update_candidate_bytes")
    )


def toolchain_version():
    result = subprocess.run(["rustc", "--version"], capture_output=True, text=True, check=True)
    return result.stdout.strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--core-example", required=True, type=Path)
    parser.add_argument("--cli-binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--iterations", type=int, default=3)
    parser.add_argument("--max-regression-percent", type=float, default=25)
    parser.add_argument("--run-update-benchmark", action="store_true")
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    args = parser.parse_args()
    if args.iterations < 3:
        raise SystemExit("use at least three iterations")

    binary = args.core_example.resolve()
    cli_binary = args.cli_binary.resolve()
    with tempfile.TemporaryDirectory(prefix="ccl-p7-performance-") as temp_name:
        scratch = Path(temp_name)
        install = run_install_samples(binary, args.iterations, scratch)
        metadata_parser = run_parser_samples(cli_binary, max(7, args.iterations), scratch)
    update_transaction = None
    if args.run_update_benchmark:
        update_transaction = run_update_benchmark(args.repo.resolve())

    not_measured = [
        "public network throughput",
        "Minecraft runtime launch/readiness",
        "allocated disk blocks",
    ]
    if not install["peak_memory"].get("available"):
        not_measured.append(
            f"peak process memory: {install['peak_memory'].get('reason', 'source unavailable')}"
        )
    report = {
        "schema_version": 1,
        "benchmark_suite": "p7_deterministic_local_performance",
        "runner_os": os.environ.get("RUNNER_OS", platform.system()),
        "runner_arch": os.environ.get("RUNNER_ARCH", platform.machine()),
        "toolchain": toolchain_version(),
        "commit": os.environ.get("GITHUB_SHA", "local"),
        "install": install,
        "metadata_parser": metadata_parser,
        "update_transaction": update_transaction,
        "not_measured": not_measured,
    }
    if update_transaction is None:
        report["not_measured"].append("update executable replacement transaction: benchmark fixture not available")
    report["observed_at"] = datetime.now(timezone.utc).isoformat()
    report["observation"] = {
        "id": os.environ.get("GITHUB_RUN_ID", f"{report['commit']}:{report['observed_at']}"),
        "observed_at": report["observed_at"],
        "benchmark_suite": report["benchmark_suite"],
        "runner_os": report["runner_os"],
        "runner_arch": report["runner_arch"],
        "toolchain": report["toolchain"],
        "build_profile": "release",
        "fixtures": {
            "install_sha256": install["fixture_sha256"],
            "metadata_parser_sha256": metadata_parser["fixture_sha256"],
            "update_candidate_bytes": update_transaction.get("candidate_bytes") if update_transaction else None,
        },
        "metrics": {
            "install_cold_ms": install["cold_elapsed_ms_median"],
            "install_warm_ms": install["warm_elapsed_ms_median"],
            "metadata_parser_ms": metadata_parser["median_ms"],
            "update_commit_ms": update_transaction.get("commit_median_ms") if update_transaction else None,
            "update_rollback_ms": update_transaction.get("rollback_median_ms") if update_transaction else None,
        },
    }
    regression = False
    if args.baseline:
        regression = compare_baseline(report, args.baseline, args.max_regression_percent)
    else:
        report["baseline_history"] = [report["observation"]]
        report["baseline_gate"] = {
            "state": "collecting",
            "minimum_observations": 3,
            "compatible_observations": 0,
            "history_limit": 5,
            "regression_limit_percent": args.max_regression_percent,
        }
        report["baseline_comparison"] = []

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(f"install cold median: {install['cold_elapsed_ms_median']} ms; warm median: {install['warm_elapsed_ms_median']} ms")
    print(f"metadata parser median: {metadata_parser['median_ms']} ms")
    if update_transaction:
        print(f"update transaction metrics: {json.dumps(update_transaction, sort_keys=True)}")
    print(f"baseline gate: {report['baseline_gate']['state']} ({report['baseline_gate']['compatible_observations']} prior compatible observations)")
    if regression:
        raise RuntimeError(f"performance regression exceeds {args.max_regression_percent:.1f}%: {report['baseline_gate']['failed_metrics']}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ValueError, KeyError, json.JSONDecodeError) as error:
        raise SystemExit(str(error))
