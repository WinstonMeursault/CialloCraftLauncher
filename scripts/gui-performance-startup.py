#!/usr/bin/env python3
"""Time real GUI readiness using the native gui_ready marker.

Each sample launches the specified app bundle directly with a fresh isolated
CCL data directory and existing read-only game fixture. This measures process
startup with UI readiness; it does not flush macOS/WebKit/system file caches.
"""
import argparse
import datetime as dt
import json
import os
import pathlib
import subprocess
import time
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=pathlib.Path, required=True)
    parser.add_argument("--game-root", type=pathlib.Path, required=True)
    parser.add_argument("--data-root", type=pathlib.Path, required=True)
    parser.add_argument("--config-root", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--kind", choices=("process-cold", "cache-warm"), required=True)
    args = parser.parse_args()
    if args.runs != 5:
        parser.error("this report workflow uses exactly five runs")
    args.app = args.app.resolve()
    args.game_root = args.game_root.resolve()
    args.data_root = args.data_root.resolve()
    args.config_root = args.config_root.resolve()
    args.output = args.output.resolve()
    executable = args.app / "Contents" / "MacOS" / "ccl-gui"
    if not executable.is_file() or not args.game_root.is_dir():
        parser.error("app executable or isolated game fixture is missing")
    if args.output.exists():
        parser.error("output already exists; choose a new evidence path")

    attempt = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]
    base = args.data_root / f"startup-{args.kind}-{attempt}"
    base.mkdir(parents=True, exist_ok=False)
    args.config_root.mkdir(parents=True, exist_ok=True)
    samples = []
    for index in range(1, 6):
        data_dir = base / f"run-{index:02}"
        data_dir.mkdir()
        marker = data_dir / "gui-ready.json"
        config = args.config_root / f"run-{index:02}.toml"
        env = os.environ.copy()
        env.update({
            "CCL_GUI_DATA_DIR": str(data_dir),
            "CCL_GUI_CONFIG_PATH": str(config),
            "CCL_GUI_GAME_DIR": str(args.game_root),
            "CCL_GUI_SMOKE_MARKER": str(marker),
        })
        started_at_utc = dt.datetime.now(dt.timezone.utc).isoformat()
        started = time.monotonic()
        process = subprocess.Popen([str(executable)], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = started + 30
            while time.monotonic() < deadline and process.poll() is None and not marker.exists():
                time.sleep(0.02)
            if not marker.exists():
                raise RuntimeError(f"GUI did not report readiness in run {index}; exit={process.poll()}")
            ready = json.loads(marker.read_text(encoding="utf-8"))
            elapsed = time.monotonic() - started
            if ready.get("status") != "ui_ready" or ready.get("pid") != process.pid:
                raise RuntimeError(f"readiness marker does not match GUI process in run {index}")
            samples.append({
                "run": index,
                "kind": args.kind,
                "started_at_utc": started_at_utc,
                "ready_elapsed_ms": round(elapsed * 1000, 3),
                "pid": process.pid,
                "build": ready.get("build"),
            })
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
            marker.unlink(missing_ok=True)

    values = sorted(row["ready_elapsed_ms"] for row in samples)
    payload = {
        "schema": "ccl.gui-startup.v1",
        "kind": args.kind,
        "cache_note": "Each run is a fresh GUI process and isolated CCL data directory. The harness does not flush macOS, LaunchServices, or WebKit caches; process-cold does not mean system-cache-cold.",
        "app_bundle": str(args.app),
        "game_fixture": str(args.game_root),
        "samples": samples,
        "summary": {"median_ms": values[len(values) // 2], "p95_ms": values[-1]},
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x", encoding="utf-8") as stream:
        json.dump(payload, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    print(json.dumps({"kind": args.kind, "runs": len(samples), "median_ms": payload["summary"]["median_ms"], "p95_ms": payload["summary"]["p95_ms"], "cache_note": payload["cache_note"]}, ensure_ascii=False))


if __name__ == "__main__":
    main()
