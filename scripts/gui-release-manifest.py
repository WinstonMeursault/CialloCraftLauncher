#!/usr/bin/env python3
"""Create the signed metadata payload for one CCL GUI updater asset."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re


UPDATER_FORMATS = {
    "x86_64-pc-windows-msvc": "nsis",
    "aarch64-apple-darwin": "app.tar.gz",
    "x86_64-unknown-linux-gnu": "appimage",
}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, choices=UPDATER_FORMATS)
    parser.add_argument("--asset", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    channel = os.environ["CCL_BUILD_CHANNEL"]
    version = os.environ["CCL_BUILD_VERSION"]
    commit = os.environ["CCL_BUILD_COMMIT"]
    branch = os.environ["CCL_BUILD_BRANCH"]
    tag = os.environ["CCL_RELEASE_TAG"]
    expected_branch = {"stable": "main", "dev": "dev"}.get(channel)
    if not expected_branch or branch != expected_branch:
        raise SystemExit("channel and branch do not match")
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise SystemExit("CCL_BUILD_COMMIT must be a full lowercase commit SHA")
    semver = r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    if not re.fullmatch(semver, version):
        raise SystemExit("CCL_BUILD_VERSION must be a valid three-part semantic version")
    expected_tag = f"v{version}" if channel == "stable" else f"dev-{commit}"
    if tag != expected_tag:
        raise SystemExit(f"release tag must bind to selected version and commit: {expected_tag}")

    extension = {
        "nsis": ".exe",
        "app.tar.gz": ".app.tar.gz",
        "appimage": ".AppImage",
    }[UPDATER_FORMATS[args.target]]
    expected_name = f"ccl-gui-{args.target}.{extension.lstrip('.')}"
    if args.asset.name != expected_name or not args.asset.is_file():
        raise SystemExit(f"updater asset must exist and be named {expected_name}")

    repository = "WinstonMeursault/CialloCraftLauncher"
    data = args.asset.read_bytes()
    manifest = {
        "schema_version": 1,
        "repository": repository,
        "product": "ccl-gui",
        "channel": channel,
        "version": version,
        "commit": commit,
        "branch": branch,
        "target": args.target,
        "os_signed": False,
        "data_schema_min": 1,
        "data_schema_max": 1,
        "asset": {
            "format": UPDATER_FORMATS[args.target],
            "name": expected_name,
            "size": len(data),
            "sha256": sha256(data),
            "url": f"https://github.com/{repository}/releases/download/{tag}/{expected_name}",
        },
    }
    output = args.output or args.asset.with_name(f"gui-manifest-{args.target}.json")
    output.write_bytes((json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8"))


if __name__ == "__main__":
    main()
