#!/usr/bin/env python3
"""Record system-wide macOS GPU utilization during a named GUI phase.

The ioreg AGXAccelerator counters are global device activity. They include the
window compositor and other applications, so compare real-UI and blank-control
runs sequentially and treat differences as observations, not causal attribution.
This tool records only the three utilization counters, timestamps, and summary
statistics; it deliberately omits device and serial identifiers.
"""
import argparse
import datetime as dt
import json
import math
import plistlib
import statistics
import subprocess
import time


METRICS = (
    "Device Utilization %",
    "Renderer Utilization %",
    "Tiler Utilization %",
)


def find_statistics(value):
    if isinstance(value, dict):
        stats = value.get("PerformanceStatistics")
        if isinstance(stats, dict) and any(key in stats for key in METRICS):
            return stats
        for child in value.values():
            found = find_statistics(child)
            if found is not None:
                return found
    elif isinstance(value, list):
        for child in value:
            found = find_statistics(child)
            if found is not None:
                return found
    return None


def sample_gpu():
    raw = subprocess.check_output(
        ["/usr/sbin/ioreg", "-a", "-r", "-c", "AGXAccelerator"], timeout=10
    )
    document = plistlib.loads(raw)
    stats = find_statistics(document)
    if stats is None:
        raise RuntimeError("AGXAccelerator PerformanceStatistics are unavailable")
    result = {}
    for metric in METRICS:
        value = stats.get(metric)
        if isinstance(value, (int, float)) and not isinstance(value, bool):
            result[metric] = float(value)
        else:
            result[metric] = None
    return result


def percentile(values, fraction):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, math.ceil(fraction * len(ordered)) - 1)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", choices=("real-ui", "blank-control"), required=True)
    parser.add_argument("--duration", type=float, default=60, help="sampling duration in seconds (default: 60)")
    parser.add_argument("--interval", type=float, default=1, help="sampling interval in seconds (default: 1)")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    if args.duration <= 0 or args.interval <= 0:
        parser.error("duration and interval must be positive")

    started = time.monotonic()
    started_at = dt.datetime.now(dt.timezone.utc).isoformat()
    samples = []
    while True:
        elapsed = time.monotonic() - started
        if elapsed >= args.duration:
            break
        timestamp = dt.datetime.now(dt.timezone.utc).isoformat()
        samples.append({
            "timestamp": timestamp,
            "elapsed_seconds": round(elapsed, 3),
            "utilization_percent": sample_gpu(),
        })
        remaining = args.duration - (time.monotonic() - started)
        if remaining > 0:
            time.sleep(min(args.interval, remaining))

    summaries = {}
    for metric in METRICS:
        values = [row["utilization_percent"][metric] for row in samples if row["utilization_percent"][metric] is not None]
        summaries[metric] = {
            "median": statistics.median(values) if values else None,
            "p95": percentile(values, .95) if values else None,
            "min": min(values) if values else None,
            "max": max(values) if values else None,
            "valid_samples": len(values),
        }
    payload = {
        "schema": "ccl.gui-gpu-utilization.v1",
        "scenario": args.scenario,
        "started_at": started_at,
        "requested_duration_seconds": args.duration,
        "interval_seconds": args.interval,
        "sample_count": len(samples),
        "metric_scope": "System-wide AGX GPU utilization, including compositor and other applications; not per-process utilization or watts.",
        "comparison_note": "Background GPU activity is not subtracted. Compare real-ui and blank-control runs sequentially; observed differences do not establish causation.",
        "summary_percent": summaries,
        "samples": samples,
    }
    with open(args.output, "x", encoding="utf-8") as stream:
        json.dump(payload, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    print(json.dumps({"scenario": args.scenario, "sample_count": len(samples), "summary_percent": summaries}, ensure_ascii=False))


if __name__ == "__main__":
    main()
