#!/usr/bin/env python3
"""Write an updater manifest for a packaged CCL release archive."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import zipfile


TARGETS = {
    "x86_64-pc-windows-msvc": "ccl.exe",
    "aarch64-apple-darwin": "ccl",
    "x86_64-unknown-linux-gnu": "ccl",
}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--target", required=True, choices=TARGETS)
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
    if not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", version):
        raise SystemExit("CCL_BUILD_VERSION must be a three-part numeric version")
    if not args.archive.is_file():
        raise SystemExit(f"archive not found: {args.archive}")
    expected_tag = f"v{version}" if channel == "stable" else f"dev-{commit}"
    if tag != expected_tag:
        raise SystemExit(f"release tag must bind to the selected version and commit: {expected_tag}")
    expected_archive = f"ccl-{args.target}.zip"
    if args.archive.name != expected_archive:
        raise SystemExit(f"archive name must be {expected_archive}")

    with zipfile.ZipFile(args.archive) as archive:
        executable_path = f"ccl-{args.target}/{TARGETS[args.target]}"
        try:
            executable = archive.read(executable_path)
        except KeyError as error:
            raise SystemExit(f"archive is missing {executable_path}") from error
    archive_bytes = args.archive.read_bytes()
    name = expected_archive
    repository = "WinstonMeursault/CialloCraftLauncher"
    manifest = {
        "schema_version": 1,
        "repository": repository,
        "product": "ccl-cli",
        "channel": channel,
        "version": version,
        "commit": commit,
        "branch": branch,
        "target": args.target,
        "data_schema_min": 1,
        "data_schema_max": 1,
        "executable_size": len(executable),
        "executable_sha256": sha256(executable),
        "asset": {
            "name": name,
            "size": len(archive_bytes),
            "sha256": sha256(archive_bytes),
            "url": f"https://github.com/{repository}/releases/download/{tag}/{name}",
        },
    }
    output = args.output or args.archive.with_name(f"manifest-{args.target}.json")
    output.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
