#!/usr/bin/env python3
"""Keep CCL's embedded core build identity and CLI package version aligned."""

import argparse
import re
from pathlib import Path


def package_version(path: Path) -> str:
    content = path.read_text(encoding="utf-8")
    try:
        section = content.split("[package]", 1)[1].split("\n[", 1)[0]
    except IndexError as error:
        raise SystemExit(f"missing [package] section in {path}") from error
    match = re.search(r'(?m)^version\s*=\s*"([^"]+)"\s*$', section)
    if not match:
        raise SystemExit(f"missing package version in {path}")
    return match.group(1)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--expected")
    parser.add_argument("--cli-version", action="store_true")
    args = parser.parse_args()
    core = package_version(Path("crates/ccl-core/Cargo.toml"))
    cli = package_version(Path("crates/ccl-cli/Cargo.toml"))
    if core != cli:
        raise SystemExit(f"ccl-core version {core} does not match ccl-cli version {cli}")
    if args.expected is not None and cli != args.expected:
        raise SystemExit(f"Cargo package version {cli} does not match expected release version {args.expected}")
    print(cli if args.cli_version else f"ccl-core and ccl-cli versions match: {cli}")


if __name__ == "__main__":
    main()
