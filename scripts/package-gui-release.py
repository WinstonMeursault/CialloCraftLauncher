#!/usr/bin/env python3
"""Collect Tauri GUI bundles under stable, product-specific release names."""

import argparse
import hashlib
import shutil
import tarfile
from pathlib import Path


FORMATS = {
    "x86_64-pc-windows-msvc": {"nsis": ".exe"},
    "aarch64-apple-darwin": {"app.tar.gz": ".app.tar.gz", "dmg": ".dmg"},
    "x86_64-unknown-linux-gnu": {"appimage": ".AppImage", "deb": ".deb"},
}


def hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, choices=FORMATS)
    parser.add_argument("--bundle-root", required=True, type=Path)
    parser.add_argument("--dist", type=Path, default=Path("dist"))
    args = parser.parse_args()

    expected = FORMATS[args.target]
    candidates = [path for path in args.bundle_root.rglob("*") if path.is_file()]
    if args.target == "aarch64-apple-darwin" and not any(
        path.name.lower().endswith(".app.tar.gz") for path in candidates
    ):
        app_bundles = [path for path in args.bundle_root.rglob("*.app") if path.is_dir()]
        if len(app_bundles) == 1:
            updater_archive = args.bundle_root / f"{app_bundles[0].name}.tar.gz"
            with tarfile.open(updater_archive, "w:gz", format=tarfile.PAX_FORMAT) as archive:
                archive.add(app_bundles[0], arcname=app_bundles[0].name, recursive=True)
            candidates.append(updater_archive)
    copied = []
    for bundle_format, suffix in expected.items():
        matches = [path for path in candidates if path.name.lower().endswith(suffix.lower())]
        if len(matches) != 1:
            raise SystemExit(
                f"expected exactly one {bundle_format} bundle under {args.bundle_root}, found {len(matches)}"
            )
        destination = args.dist / f"ccl-gui-{args.target}.{suffix.lstrip('.')}"
        args.dist.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(matches[0], destination)
        digest = hash_file(destination)
        (args.dist / f"{destination.name}.sha256").write_text(
            f"{digest}  {destination.name}\n", encoding="ascii"
        )
        copied.append(destination)
    print("\n".join(str(path) for path in copied))


if __name__ == "__main__":
    main()
