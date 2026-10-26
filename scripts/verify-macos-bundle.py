#!/usr/bin/env python3
"""Reject incomplete macOS bundle signatures before publishing either payload."""

import argparse
from pathlib import Path
import subprocess
import tempfile


def verify(app: Path) -> None:
    # A linker-signed Mach-O alone is insufficient: Info.plist and resources
    # must belong to the signed application bundle as well.
    subprocess.run(
        ["codesign", "--verify", "--deep", "--strict", "--verbose=2", str(app)],
        check=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--dmg", required=True, type=Path)
    args = parser.parse_args()
    verify(args.app)
    with tempfile.TemporaryDirectory(prefix="ccl-signature-check-") as temporary:
        root = Path(temporary)
        unpacked = root / "archive"
        unpacked.mkdir()
        # Only locally generated release artifacts are inspected here.
        subprocess.run(
            ["tar", "-xzf", str(args.archive.resolve()), "-C", str(unpacked)],
            check=True,
        )
        apps = list(unpacked.glob("*.app"))
        if len(apps) != 1:
            raise SystemExit("updater archive must contain exactly one application")
        verify(apps[0])
        mount = root / "image"
        mount.mkdir()
        mounted = False
        try:
            # The bundled project license can prompt during attachment.
            result = subprocess.run(
                ["hdiutil", "attach", "-readonly", "-nobrowse", "-noautoopen",
                 "-mountpoint", str(mount), str(args.dmg.resolve())],
                input="Y\n", text=True, capture_output=True,
            )
            mounted = mount.is_mount()
            if result.returncode:
                raise SystemExit(f"cannot inspect release DMG: {result.stderr}")
            apps = list(mount.glob("*.app"))
            if len(apps) != 1:
                raise SystemExit("DMG must contain exactly one application")
            verify(apps[0])
        finally:
            if mounted:
                subprocess.run(["hdiutil", "detach", str(mount)], check=True)
    print("Application, updater archive and DMG bundle signatures verified.")


if __name__ == "__main__":
    main()
