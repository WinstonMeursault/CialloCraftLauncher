#!/usr/bin/env python3
"""Launch an isolated real desktop window and require post-render IPC readiness."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    executable = args.executable.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="ccl-gui-smoke-") as directory:
        root = Path(directory).resolve()
        data = root / "gui"
        data.mkdir()
        marker = data / "ready.json"
        config = root / "config.json"
        config.write_text(json.dumps({"downloads": {"offline": True}}))
        game = root / ".minecraft"
        game.mkdir()
        env = dict(os.environ, CCL_GUI_DATA_DIR=str(data), CCL_GUI_CONFIG_PATH=str(config),
                   CCL_GUI_GAME_DIR=str(game), CCL_GUI_SMOKE_MARKER=str(marker))
        start = time.monotonic()
        with (root / "stderr.log").open("w") as errors:
            process = subprocess.Popen([str(executable)], env=env, stdout=subprocess.DEVNULL, stderr=errors,
                                       start_new_session=os.name != "nt")
            try:
                while not marker.exists():
                    if process.poll() is not None:
                        raise RuntimeError(f"Desktop exited before UI readiness ({process.returncode}): " + (root / "stderr.log").read_text()[-4000:])
                    if time.monotonic() - start > 45:
                        raise RuntimeError("Desktop did not complete rendered bootstrap IPC within 45 seconds")
                    time.sleep(0.05)
                report = json.loads(marker.read_text())
                if report.get("status") != "ui_ready":
                    raise RuntimeError("Unexpected readiness marker")
                report.update(elapsed_seconds=round(time.monotonic() - start, 3), platform=os.sys.platform,
                              scope="real WebView rendered bootstrap, isolated offline root; no physical gameplay claim")
                args.output.parent.mkdir(parents=True, exist_ok=True)
                args.output.write_text(json.dumps(report, indent=2) + "\n")
                print(json.dumps(report))
            finally:
                if os.name == "nt":
                    subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], capture_output=True)
                else:
                    try:
                        os.killpg(process.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == "__main__":
    main()
