#!/usr/bin/env python3
"""Bundle paired Rust, production JavaScript, project and public GUI notices."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess


def production_packages(root):
    result = {}

    def resolve(name, location):
        for ancestor in (location, *location.parents):
            candidate = ancestor / "node_modules" / name / "package.json"
            if candidate.is_file():
                return candidate.resolve()
        raise RuntimeError(f"Missing installed production dependency: {name}")

    def walk(manifest):
        manifest = manifest.resolve()
        data = json.loads(manifest.read_text(encoding="utf-8"))
        key = (data["name"], data["version"])
        if key in result:
            return
        result[key] = manifest.parent
        for name in data.get("dependencies", {}):
            walk(resolve(name, manifest.parent))

    for name in json.loads((root / "package.json").read_text(encoding="utf-8"))["dependencies"]:
        walk(resolve(name, root))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    output = repo / "target/gui-notices"
    subprocess.run(["python3", str(repo / "scripts/third-party-notices.py"), "--target", args.target,
                    "--package", "ccl-gui", "--output-dir", str(output)], check=True)
    packages = production_packages(repo / "apps/ccl-gui")
    for language in ("en", "zh-CN"):
        inventory = output / f"THIRD-PARTY.{language}.md"
        bundle = output / f"THIRD-PARTY-LICENSES.{language}.md"
        with inventory.open("a", encoding="utf-8") as listing, bundle.open("a", encoding="utf-8") as texts:
            title = "Production JavaScript dependencies" if language == "en" else "生产 JavaScript 依赖"
            listing.write(f"\n## {title}\n\n")
            texts.write(f"\n## {title}\n\n")
            for (name, version), directory in sorted(packages.items()):
                data = json.loads((directory / "package.json").read_text(encoding="utf-8"))
                files = [p for p in directory.iterdir() if p.is_file() and re.match(r"^(LICENSE|LICENCE|COPYING|NOTICE)([.-].*)?$", p.name, re.I)]
                if not files:
                    raise RuntimeError(f"Missing original license text for {name}@{version}")
                anchor = "npm-" + hashlib.sha256(f"{name}@{version}".encode()).hexdigest()[:20]
                listing.write(f"- **{name} {version}** — {data.get('license', 'SEE LICENSE')}; [license](THIRD-PARTY-LICENSES.{language}.md#{anchor})\n")
                texts.write(f'\n<a id="{anchor}"></a>\n\n### {name} {version}\n\n')
                for path in sorted(files):
                    text = path.read_text(encoding="utf-8")
                    fence = "`" * max(3, max((len(s) for s in re.findall(r"`+", text)), default=0) + 1)
                    texts.write(f"{path.name}\n\n{fence}text\n{text}\n{fence}\n")
        shutil.copyfile(repo / f"README.{language}.md" if (repo / f"README.{language}.md").exists() else repo / "README.md", output / f"README.{language}.md")
        for document in (repo / "docs").glob(f"*.{language}.md"):
            shutil.copyfile(document, output / document.name)
    shutil.copyfile(repo / "LICENSE", output / "LICENSE")
    print(f"GUI notices: Rust and {len(packages)} production JavaScript packages")


if __name__ == "__main__":
    main()
