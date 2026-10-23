#!/usr/bin/env python3
"""Probe fixed public metadata samples through the shipped CCL CLI."""

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


SAMPLES = [
    ("fabric", "fabric", "1.21.1"),
    ("forge_modern", "forge", "1.21.1"),
    ("forge_legacy", "forge", "1.12.2"),
    ("neoforge", "neoforge", "1.21.1"),
    ("quilt", "quilt", "1.21.4"),
    ("legacyfabric", "legacyfabric", "1.8.9"),
    ("optifine", "optifine", "1.20.1"),
]


def run_json(binary: Path, prefix, args, timeout):
    command = [str(binary), "--json", *prefix, *args]
    started = time.perf_counter()
    result = subprocess.run(command, capture_output=True, text=True, timeout=timeout, check=False)
    elapsed_ms = (time.perf_counter() - started) * 1000
    if result.returncode:
        raise RuntimeError(
            f"command failed ({result.returncode}, {elapsed_ms:.0f} ms): {command!r}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    try:
        document = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"command returned invalid JSON: {command!r}\n{result.stdout}\n{result.stderr}") from error
    if document.get("ok") is not True:
        raise RuntimeError(f"command reported failure: {command!r}\n{document!r}")
    return document, round(elapsed_ms, 1)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--timeout-seconds", type=int, default=90)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        raise SystemExit(f"CCL binary not found: {binary}")

    samples = {}
    with tempfile.TemporaryDirectory(prefix="ccl-upstream-") as temporary:
        prefix = ["--game-dir", str(Path(temporary) / "game")]
        versions, elapsed = run_json(binary, prefix, ["versions", "--limit", "0"], args.timeout_seconds)
        latest = versions.get("latest", {})
        all_versions = versions.get("versions", [])
        version_by_id = {item.get("id"): item for item in all_versions if isinstance(item, dict)}
        for key, expected_type in (("release", "release"), ("snapshot", "snapshot")):
            version_id = latest.get(key)
            selected = version_by_id.get(version_id)
            if not version_id or not selected or selected.get("type") != expected_type:
                raise RuntimeError(f"vanilla latest.{key} is missing or has unexpected type: {latest!r}")
            samples[f"vanilla_latest_{key}"] = {"id": version_id, "type": expected_type}
        known = version_by_id.get("1.20.1")
        if not known or known.get("type") != "release":
            raise RuntimeError("known vanilla release sample 1.20.1 is missing from the public manifest")
        samples["vanilla_known_release"] = {"id": "1.20.1", "type": known["type"]}
        samples["vanilla_catalog"] = {
            "entry_count": len(all_versions),
            "elapsed_ms": elapsed,
            "latest": latest,
        }

    loader_results = {}
    for sample_name, loader, minecraft in SAMPLES:
        result, elapsed = run_json(
            binary,
            [],
            ["loaders", loader, minecraft, "--limit", "3"],
            args.timeout_seconds,
        )
        if result.get("minecraft") != minecraft:
            raise RuntimeError(f"{loader} catalog response does not match Minecraft {minecraft}")
        entries = result.get("loader_versions")
        if not isinstance(entries, list) or not entries:
            raise RuntimeError(f"{loader} catalog returned no entries for Minecraft {minecraft}")
        for entry in entries:
            if not isinstance(entry, dict) or not isinstance(entry.get("version"), str) or not entry["version"]:
                raise RuntimeError(f"{loader} catalog returned malformed version data: {entries!r}")
        loader_results[sample_name] = {
            "loader": loader,
            "minecraft": minecraft,
            "returned": len(entries),
            "versions": entries,
            "elapsed_ms": elapsed,
        }

    report = {
        "schema_version": 1,
        "suite": "p7_upstream_metadata_regression",
        "observed_at": datetime.now(timezone.utc).isoformat(),
        "commit": os.environ.get("GITHUB_SHA", "local"),
        "runner_os": os.environ.get("RUNNER_OS", os.name),
        "runner_arch": os.environ.get("RUNNER_ARCH", "unknown"),
        "vanilla": {key: value for key, value in samples.items() if key.startswith("vanilla_")},
        "loaders": loader_results,
        "side_effects": "temporary version metadata cache only; no account, instance, or game installation writes",
        "public_metadata_cli_queries": 1 + len(SAMPLES),
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ValueError, subprocess.TimeoutExpired) as error:
        raise SystemExit(str(error))
