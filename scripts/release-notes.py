#!/usr/bin/env python3
"""Render the reviewed bilingual release bodies from the consolidated guides."""

import argparse
from pathlib import Path
import re


def render(root, version):
    sections = []
    for language, title in (("en", "English"), ("zh-CN", "简体中文")):
        text = (root / f"docs/development.{language}.md").read_text(encoding="utf-8")
        start, end = "<!-- release-notes:start -->", "<!-- release-notes:end -->"
        if text.count(start) != 1 or text.count(end) != 1:
            raise ValueError(f"{language}: expected one reviewed release-notes section")
        body = text.split(start, 1)[1].split(end, 1)[0].strip()
        body = "\n".join(re.sub(r"^> ?", "", line) for line in body.splitlines())
        # The guide's title must match the package version; never silently
        # publish stale notes after bumping a crate version.
        if f"CialloCraftLauncher {version}" not in body:
            raise ValueError(f"{language}: release notes do not match version {version}")
        body = body.replace(" (draft)", "").replace("（草稿）", "")
        sections.append(f"## {title}\n\n{body}")
    return "\n\n---\n\n".join(sections) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", args.version):
        parser.error("version must be a three-part numeric version")
    body = render(Path(__file__).resolve().parent.parent, args.version)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(body, encoding="utf-8")


if __name__ == "__main__":
    main()
