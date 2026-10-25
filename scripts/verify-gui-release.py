#!/usr/bin/env python3
"""Validate downloaded GUI release metadata and updater payload hashes."""

import argparse
import base64
import hashlib
import json
from pathlib import Path


EXPECTED = {
    "x86_64-pc-windows-msvc": ("nsis", "exe", ()),
    "aarch64-apple-darwin": ("app.tar.gz", "app.tar.gz", ("dmg",)),
    "x86_64-unknown-linux-gnu": ("appimage", "AppImage", ("deb",)),
}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--channel", choices=("stable", "dev"), required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--branch", required=True)
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()

    manifests = sorted(args.directory.glob("gui-manifest-*.json"))
    if len(manifests) != len(EXPECTED):
        raise SystemExit("expected exactly three GUI manifests")
    expected_tag = f"v{args.version}" if args.channel == "stable" else f"dev-{args.commit}"
    if args.tag != expected_tag:
        raise SystemExit("GUI release tag does not match channel, version, and commit")

    for path in manifests:
        manifest = json.loads(path.read_bytes())
        target = manifest.get("target")
        if target not in EXPECTED or path.name != f"gui-manifest-{target}.json":
            raise SystemExit(f"unexpected GUI manifest identity: {path.name}")
        asset_format, extension, distribution_formats = EXPECTED[target]
        asset = manifest.get("asset", {})
        name = f"ccl-gui-{target}.{extension}"
        if (
            manifest.get("schema_version") != 1
            or manifest.get("repository") != "WinstonMeursault/CialloCraftLauncher"
            or manifest.get("product") != "ccl-gui"
            or manifest.get("channel") != args.channel
            or manifest.get("version") != args.version
            or manifest.get("commit") != args.commit
            or manifest.get("branch") != args.branch
            or manifest.get("os_signed") is not False
            or manifest.get("data_schema_min") != 1
            or manifest.get("data_schema_max") != 1
            or asset.get("format") != asset_format
            or asset.get("name") != name
            or asset.get("url")
            != f"https://github.com/WinstonMeursault/CialloCraftLauncher/releases/download/{args.tag}/{name}"
        ):
            raise SystemExit(f"GUI manifest identity or updater format mismatch: {path.name}")

        asset_path = args.directory / name
        data = asset_path.read_bytes()
        if len(data) != asset.get("size") or hashlib.sha256(data).hexdigest() != asset.get("sha256"):
            raise SystemExit(f"GUI updater payload digest mismatch: {name}")
        for distribution_format in distribution_formats:
            distribution = args.directory / f"ccl-gui-{target}.{distribution_format}"
            if not distribution.is_file() or distribution.stat().st_size == 0:
                raise SystemExit(f"missing GUI distribution package: {distribution.name}")
        signature = path.with_name(path.name + ".sig")
        try:
            encoded_signature = signature.read_text(encoding="ascii").strip()
            decoded_signature = base64.b64decode(encoded_signature, validate=True).decode("ascii")
        except (OSError, UnicodeError, ValueError) as error:
            raise SystemExit(f"invalid GUI manifest signature asset: {signature.name}") from error
        comments = [line.removeprefix("trusted comment: ") for line in decoded_signature.splitlines() if line.startswith("trusted comment: ")]
        versions = [field.removeprefix("version:") for comment in comments for field in comment.split("\t") if field.startswith("version:")]
        if len(comments) != 1 or versions != [args.version]:
            raise SystemExit(f"GUI manifest signature does not bind its app version: {signature.name}")


if __name__ == "__main__":
    main()
