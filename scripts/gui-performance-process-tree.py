#!/usr/bin/env python3
"""Sample a macOS GUI process tree for a manually launched release app.

This records RSS and attempts macOS footprint. Descendants are attributed by
process ancestry; PPID-1 WebKit helpers are attributed only when lsof finds an
open path with the exact app bundle ID. Output is raw evidence, not a pass/fail
claim.
"""
import argparse
import datetime as dt
import json
import math
import os
import re
import statistics
import subprocess
import time


def process_table():
    raw = subprocess.check_output(["ps", "-axo", "pid=,ppid=,%cpu=,rss=,comm="], text=True)
    rows = {}
    for line in raw.splitlines():
        parts = line.strip().split(None, 4)
        if len(parts) != 5:
            continue
        pid, ppid, cpu, rss, command = parts
        try:
            rows[int(pid)] = {"pid": int(pid), "ppid": int(ppid), "cpu_percent": float(cpu), "rss_kib": int(rss), "command": command}
        except ValueError:
            continue
    return rows


def descendants(rows, root):
    found = {root}
    changed = True
    while changed:
        changed = False
        for pid, row in rows.items():
            if row["ppid"] in found and pid not in found:
                found.add(pid)
                changed = True
    return sorted(found)


def possible_reparented_webkit(rows, tree_pids):
    """Find WebKit-looking launchd children, but do not claim ownership."""
    return [
        row for pid, row in rows.items()
        if pid not in tree_pids and row["ppid"] == 1
        and any(marker in row["command"].lower() for marker in ("webkit", "webcontent", "networking"))
    ]


def app_specific_paths(pid, bundle_id):
    """Return only bundle-specific open paths; never persist arbitrary lsof names."""
    try:
        result = subprocess.run(
            ["/usr/sbin/lsof", "-Fn", "-p", str(pid)],
            capture_output=True, text=True, timeout=10, check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return []
    if result.returncode:
        return []
    marker = f"/{bundle_id.lower()}"
    paths = []
    for line in result.stdout.splitlines():
        if not line.startswith("n"):
            continue
        path = line[1:]
        lowered = path.lower()
        index = lowered.find(marker)
        suffix = lowered[index + len(marker):] if index >= 0 else None
        if suffix is not None and (not suffix or suffix.startswith("/")):
            paths.append(path)
    return sorted(set(paths))


def footprint_attempt(pid):
    """Try macOS footprint once; retain permission failures as explicit evidence."""
    try:
        result = subprocess.run(
            ["/usr/bin/footprint", "--pid", str(pid), "--format", "bytes"],
            capture_output=True, text=True, timeout=30, check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"available": False, "reason": str(error)[:240]}
    output = (result.stdout + result.stderr).strip()
    if result.returncode:
        return {"available": False, "exit_code": result.returncode, "reason": output[-400:]}
    current = re.search(r"phys_footprint:\s*([\d,]+)\s*B", output)
    peak = re.search(r"phys_footprint_peak:\s*([\d,]+)\s*B", output)
    current_bytes = int(current.group(1).replace(",", "")) if current else None
    peak_bytes = int(peak.group(1).replace(",", "")) if peak else None
    return {
        "available": True,
        "phys_footprint_bytes": current_bytes,
        "phys_footprint_peak_bytes": peak_bytes,
        "output": output[-2000:],
        "metric": "macOS phys_footprint; current per-process footprint, not RSS",
    }


def percentile(values, fraction):
    if not values:
        return None
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, math.ceil(fraction * len(ordered)) - 1)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, required=True, help="PID of the release GUI main process")
    parser.add_argument("--bundle-id", default="io.github.winstonmeursault.ccl.gui", help="Expected GUI bundle identifier used to match app-specific WebKit data paths")
    parser.add_argument("--scenario", choices=("idle-visible", "idle-hidden", "log-pressure-visible", "log-pressure-hidden"), required=True)
    parser.add_argument("--seconds", type=float, default=60)
    parser.add_argument("--interval", type=float, default=1)
    parser.add_argument("--output", required=True, help="JSON evidence path")
    args = parser.parse_args()
    if args.seconds <= 0 or args.interval <= 0:
        parser.error("seconds and interval must be positive")

    samples = []
    footprint = None
    candidate_evidence = {}
    started = time.monotonic()
    while time.monotonic() - started < args.seconds:
        timestamp = dt.datetime.now(dt.timezone.utc).isoformat()
        rows = process_table()
        pids = descendants(rows, args.pid)
        webkit_candidates = possible_reparented_webkit(rows, set(pids))
        for candidate in webkit_candidates:
            pid = candidate["pid"]
            if pid not in candidate_evidence:
                paths = app_specific_paths(pid, args.bundle_id)
                candidate_evidence[pid] = paths
        proven_webkit_pids = {pid for pid, paths in candidate_evidence.items() if paths and pid in rows}
        attributed_pids = sorted(set(pids) | proven_webkit_pids)
        present = [dict(rows[pid], attribution=("descendant" if pid in pids else "bundle_specific_webkit_path"), evidence_paths=(candidate_evidence.get(pid, []) if pid not in pids else [])) for pid in attributed_pids if pid in rows]
        webkit_candidates = [dict(row, app_specific_open_paths=candidate_evidence.get(row["pid"], []), attributed=row["pid"] in proven_webkit_pids) for row in webkit_candidates]
        samples.append({
            "timestamp": timestamp,
            "elapsed_seconds": round(time.monotonic() - started, 3),
            "app_processes_and_bundle_attributed_webkit": present,
            "reparented_webkit_candidates": webkit_candidates,
            "cpu_percent_one_core": sum(row["cpu_percent"] for row in present) / 100,
            "rss_mib": sum(row["rss_kib"] for row in present) / 1024,
            "possible_webkit_candidate_rss_mib_unattributed": sum(row["rss_kib"] for row in webkit_candidates) / 1024,
        })
        if args.pid not in rows:
            break
        time.sleep(min(args.interval, max(0, args.seconds - (time.monotonic() - started))))

    cpus = [sample["cpu_percent_one_core"] for sample in samples]
    rss = [sample["rss_mib"] for sample in samples]
    final_rows = process_table()
    footprint_rows = {args.pid: {"attribution": "app main process", "result": footprint_attempt(args.pid) if args.pid in final_rows else {"available": False, "reason": "root process exited before footprint sample"}}}
    proven_pids = {pid for pid, paths in candidate_evidence.items() if paths}
    for pid in sorted(proven_pids):
        row = final_rows.get(pid)
        footprint_rows[pid] = {
            "attribution": "bundle-specific WebKit data path" if row else "bundle-specific path observed; process exited before footprint sample",
            "evidence_paths": candidate_evidence[pid],
            "result": footprint_attempt(pid) if row else {"available": False, "reason": "process exited before footprint sample"},
        }
    valid_footprints = [entry["result"]["phys_footprint_bytes"] for entry in footprint_rows.values() if entry["result"].get("phys_footprint_bytes") is not None]
    all_attributed_live = all(pid in final_rows for pid in {args.pid} | proven_pids)
    payload = {
        "schema": "ccl.gui-process-tree.v1",
        "scenario": args.scenario,
        "started_at": samples[0]["timestamp"] if samples else None,
        "requested_seconds": args.seconds,
        "sample_count": len(samples),
        "root_pid": args.pid,
        "bundle_id": args.bundle_id,
        "logical_cpu_count": os.cpu_count(),
        "memory_metric": "sum of process RSS from ps, MiB; not private memory; ancestry only",
        "cpu_metric": "sum of ps %CPU / 100, normalized to one logical core; includes process descendants plus PPID-1 WebKit processes only when lsof proves an open path containing the exact GUI bundle identifier",
        "physical_footprint": {
            "sampled_after_process_tree_run": True,
            "per_process": footprint_rows,
            "attributed_process_total_bytes": sum(valid_footprints) if valid_footprints else None,
            "attributed_process_total_mib": sum(valid_footprints) / (1024 * 1024) if valid_footprints else None,
            "available_processes_complete": all_attributed_live and len(valid_footprints) == len(footprint_rows),
            "limitations": "Sum of macOS per-process phys_footprint for the app plus bundle-path-attributed WebKit processes. Processes lacking app-specific path evidence and any inaccessible/exited PIDs are excluded; this is not RSS or a complete coalition total.",
        },
        "webkit_attribution": "PPID-1 WebKit/WebContent/Networking processes count in app totals only when lsof shows a path containing the exact GUI bundle identifier; other candidates are listed separately. Process name alone is not ownership evidence.",
        "summary": {
            "cpu_one_core_median": statistics.median(cpus) if cpus else None,
            "cpu_one_core_p95": percentile(cpus, .95),
            "rss_mib_median": statistics.median(rss) if rss else None,
            "rss_mib_p95": percentile(rss, .95),
        },
        "samples": samples,
        "limitations": ["bundle-path evidence may miss WebKit processes that never open app-specific data paths", "system and background noise is not subtracted", "GPU and power are not measured", "footprint output is stored verbatim and may be unavailable or need manual interpretation; RSS remains the only normalized memory series"],
    }
    with open(args.output, "w", encoding="utf-8") as stream:
        json.dump(payload, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    print(json.dumps({key: payload[key] for key in ("schema", "scenario", "sample_count", "summary", "limitations")}, ensure_ascii=False))


if __name__ == "__main__":
    main()
