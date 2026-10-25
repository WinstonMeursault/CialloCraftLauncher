#!/usr/bin/env python3
"""Exercise signed GUI package replacement, rollback and crash recovery in CI.

Preparation creates isolated fixtures; a separate Actions step signs their exact
manifests using repository Secrets. This script never reads private key material.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tarfile
import time


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def tree(path):
    if path.is_file():
        return {"": sha(path)}
    return {str(p.relative_to(path)): sha(p) for p in path.rglob("*") if p.is_file()}


def stop_ui(root):
    marker = root / "gui/ready.json"
    if marker.exists():
        pid = json.loads(marker.read_text())["pid"]
        if os.name == "nt":
            subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"], capture_output=True)
        else:
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        marker.unlink(missing_ok=True)
        time.sleep(1)


def wait(root, phases):
    journal = root / "gui/update/journal.json"
    deadline = time.monotonic() + 150
    while time.monotonic() < deadline:
        data = json.loads(journal.read_text()) if journal.exists() else {}
        if data.get("phase") in phases:
            return data
        time.sleep(.1)
    raise RuntimeError(f"Update did not reach {phases}; last phase: {data.get('phase')}")


def environment(root):
    return dict(os.environ, CCL_GUI_DATA_DIR=str(root / "gui"), CCL_GUI_CONFIG_PATH=str(root / "config.json"),
                CCL_GUI_GAME_DIR=str(root / ".minecraft"), CCL_GUI_SMOKE_MARKER=str(root / "gui/ready.json"))


def prepare(args):
    source = args.bundle.resolve()
    payload = args.package.resolve()
    if "windows" in args.target:
        # Installing three times can uninstall a previous fixture via NSIS's
        # registered product. Install once, then clone that isolated baseline.
        seed=(args.root / "_seed").resolve()
        seed.mkdir(parents=True)
        subprocess.run([str(payload), "/S", f"/D={seed}"], check=True, timeout=120)
        source=seed
    for name in ("success", "failure", "interrupted"):
        root = (args.root / name).resolve()
        root.mkdir(parents=True)
        (root / "gui").mkdir()
        (root / ".minecraft").mkdir()
        (root / ".minecraft/sentinel.txt").write_text("user data must survive every update\n")
        (root / "config.json").write_text(json.dumps({"downloads": {"offline": True}}))
        target = root / ("CialloCraftLauncher.app" if args.target.endswith("darwin") else "installed" if "windows" in args.target else "CialloCraftLauncher.AppImage")
        if "windows" in args.target:
            shutil.copytree(source,target)
            (target / "acceptance-original.txt").write_text("original installation\n")
            with (target / "ccl-gui.exe").open("ab") as file:
                file.write(b"CCL original acceptance marker\n")
        elif source.is_dir():
            shutil.copytree(source, target)
            (target / "acceptance-original.txt").write_text("original installation\n")
        else:
            shutil.copy2(source, target)
            with target.open("ab") as file:
                file.write(b"CCL original acceptance marker\n")
        before = tree(target)
        (root / "before.json").write_text(json.dumps(before, sort_keys=True))
        extension = ".app.tar.gz" if args.target.endswith("darwin") else ".exe" if "windows" in args.target else ".AppImage"
        package = root / f"ccl-gui-{args.target}{extension}"
        if name == "failure":
            if args.target.endswith("darwin"):
                bad = root / "bad/CialloCraftLauncher.app/Contents/MacOS"
                bad.mkdir(parents=True)
                (bad / "ccl-gui").write_bytes(b"invalid executable\n")
                (bad / "ccl-gui").chmod(0o755)
                (bad.parent / "Info.plist").write_text("not an application")
                with tarfile.open(package, "w:gz") as archive:
                    archive.add(bad.parent.parent, arcname="CialloCraftLauncher.app")
            else:
                package.write_bytes(b"MZ invalid signed acceptance fixture\n")
        else:
            shutil.copyfile(payload, package)
        env = dict(os.environ, CCL_BUILD_CHANNEL="dev", CCL_BUILD_VERSION=args.version,
                   CCL_BUILD_COMMIT=args.commit, CCL_BUILD_BRANCH="dev", CCL_RELEASE_TAG=f"dev-{args.commit}")
        subprocess.run(["python3", "scripts/gui-release-manifest.py", "--target", args.target, "--asset", str(package)], env=env, check=True)
        (root / "installation.json").write_text(json.dumps({"target": str(target)}))


def run(args):
    results = []
    for name in ("success", "failure", "interrupted"):
        root = (args.root / name).resolve()
        target = Path(json.loads((root / "installation.json").read_text())["target"])
        before = json.loads((root / "before.json").read_text())
        manifest = root / f"gui-manifest-{args.target}.json"
        package = root / json.loads(manifest.read_text())["asset"]["name"]
        state = root / "gui/update"
        env = environment(root)
        command = [str(args.driver.resolve()), "stage-only" if name == "interrupted" else "stage", str(target), str(state), str(manifest), str(manifest) + ".sig", str(package)]
        started = time.monotonic()
        try:
            subprocess.run(command, env=env, check=True, timeout=30)
            if name == "success":
                wait(root, {"applied"})
                if tree(target) == before:
                    raise RuntimeError("Successful update did not replace the original package tree")
                stop_ui(root)
                subprocess.run([str(args.driver.resolve()), "rollback", str(target), str(state)], env=env, check=True, timeout=30)
                final = wait(root, {"rolled_back"})
            elif name == "failure":
                final = wait(root, {"rolled_back"})
                if not final.get("last_error"):
                    raise RuntimeError("Failed candidate did not record rollback reason")
            else:
                path = state / "journal.json"
                journal = json.loads(path.read_text())
                journal["phase"] = "replacing"
                path.write_text(json.dumps(journal))
                target.rename(target.parent / f".ccl-gui-{journal['token']}-previous")
                subprocess.run([str(args.driver.resolve()), "recover", str(target), str(state)], env=env, check=True, timeout=30)
                final = wait(root, {"rolled_back"})
            if tree(target) != before:
                raise RuntimeError(f"{name}: original package not restored exactly")
            if (root / ".minecraft/sentinel.txt").read_text() != "user data must survive every update\n":
                raise RuntimeError("Game data changed during application update")
            results.append({"scenario": name, "passed": True, "phase": final["phase"], "elapsed_seconds": round(time.monotonic() - started, 3)})
        finally:
            # Rollback launches the previous UI after persisting the journal.
            deadline = time.monotonic() + 15
            while not (root / "gui/ready.json").exists() and time.monotonic() < deadline:
                time.sleep(.1)
            stop_ui(root)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({"target": args.target, "results": results}, indent=2) + "\n")
    print(json.dumps(results))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("prepare", "run"))
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--bundle", type=Path)
    parser.add_argument("--package", type=Path)
    parser.add_argument("--version")
    parser.add_argument("--commit")
    parser.add_argument("--driver", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.mode == "prepare":
        prepare(args)
    else:
        run(args)


if __name__ == "__main__":
    main()
