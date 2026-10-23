#!/usr/bin/env python3
"""Build the CLI's target-specific third-party dependency notice bundle."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys


LICENSE_NAME = re.compile(
    r"^(?:LICENSES?|LICENCES?|COPYING|COPYRIGHTS?|NOTICE|UNLICENSE|PATENTS|"
    r"THIRD[-_ ]PARTY(?:[-_ ](?:LICENSES?|NOTICES?))?)(?:[._ -].*)?$",
    re.IGNORECASE,
)


def command_output(command, cwd):
    result = subprocess.run(command, cwd=cwd, check=False, capture_output=True, text=True)
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip()
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(command)}\n{detail}")
    return result.stdout.strip()


def dependency_packages(metadata):
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    workspace_members = set(metadata["workspace_members"])
    root = next(
        (package for package in metadata["packages"] if package["name"] == "ccl-cli"),
        None,
    )
    if root is None or root["id"] not in nodes:
        raise RuntimeError("Cargo metadata did not resolve the ccl-cli package")

    reached = {root["id"]}
    pending = [root["id"]]
    while pending:
        node_id = pending.pop()
        for dependency in nodes[node_id]["deps"]:
            # Cargo represents normal dependencies with kind=null.
            if not any(kind.get("kind") in (None, "build") for kind in dependency["dep_kinds"]):
                continue
            package_id = dependency["pkg"]
            if package_id not in reached:
                reached.add(package_id)
                pending.append(package_id)

    selected = [packages[package_id] for package_id in reached if package_id not in workspace_members]
    return sorted(selected, key=lambda item: (item["name"].casefold(), item["version"], item.get("source") or ""))


def license_files(package):
    manifest = Path(package["manifest_path"])
    root = manifest.parent
    found = set()
    declared = package.get("license_file")
    if declared:
        path = Path(declared)
        if not path.is_absolute():
            path = root / path
        if path.is_file():
            found.add(path)

    for path in root.rglob("*"):
        if path.is_file() and LICENSE_NAME.match(path.name):
            found.add(path)

    usable = []
    non_text = []
    for path in sorted(found, key=lambda item: item.as_posix().casefold()):
        try:
            path.read_bytes().decode("utf-8")
        except UnicodeDecodeError:
            non_text.append(path)
            continue
        usable.append(path)
    return root, usable, non_text


def package_identity(package, workspace):
    source = package.get("source")
    if source:
        identity = source
    else:
        manifest = Path(package["manifest_path"]).parent
        try:
            identity = "path:" + manifest.relative_to(workspace).as_posix()
        except ValueError:
            identity = "path:" + manifest.name
    return identity


def source_url(package, workspace):
    source = package.get("source")
    if source and source.startswith("registry+") and "crates.io-index" in source:
        return f"https://crates.io/crates/{package['name']}/{package['version']}"
    if source:
        return source
    manifest = Path(package["manifest_path"]).parent
    try:
        return "workspace path: " + manifest.relative_to(workspace).as_posix()
    except ValueError:
        return "local path dependency: " + manifest.name


def markdown_cell(value):
    return str(value).replace("|", "\\|").replace("\n", " ").strip()


def render_inventory(language, target, root_package, cargo_version, rustc_version, lock_hash, entries):
    if language == "en":
        title = "Third-party notices"
        scope = (
            f"This inventory lists the normal and build-time third-party dependencies "
            f"resolved for `{root_package['name']} {root_package['version']}` on `{target}`. "
            "License and notice files are copied verbatim from the corresponding crate sources."
        )
        metadata = (
            f"Cargo: `{cargo_version}`; Rust: `{rustc_version}`; "
            f"Cargo.lock SHA-256: `{lock_hash}`."
        )
        headers = ("Crate", "Version", "Cargo license", "Source", "Bundled texts")
    else:
        title = "第三方许可证与声明"
        scope = (
            f"此清单列出为 `{root_package['name']} {root_package['version']}` 在 `{target}` "
            "解析的普通依赖和构建依赖。许可证与声明文本从对应的 crate 源码中原样复制。"
        )
        metadata = (
            f"Cargo：`{cargo_version}`；Rust：`{rustc_version}`；"
            f"Cargo.lock SHA-256：`{lock_hash}`。"
        )
        headers = ("Crate", "版本", "Cargo 许可证字段", "来源", "随包文本")

    lines = [f"# {title}", "", scope, "", metadata, "", "| " + " | ".join(headers) + " |",
             "| " + " | ".join("---" for _ in headers) + " |"]
    for entry in entries:
        source = entry["source_url"]
        source_cell = f"<{source}>" if source.startswith("https://") else source
        text_paths = ", ".join(f"[`{path}`]({path})" for path in entry["files"])
        cells = (
            f"`{entry['name']}`",
            f"`{entry['version']}`",
            f"`{entry['license']}`",
            source_cell,
            text_paths,
        )
        lines.append("| " + " | ".join(markdown_cell(cell) for cell in cells) + " |")
    lines.append("")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, help="Rust target triple for the packaged executable")
    parser.add_argument("--output-dir", required=True, type=Path, help="Package staging directory")
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    repo = args.repo.resolve()
    output_dir = args.output_dir.resolve()

    metadata_json = command_output(
        ["cargo", "metadata", "--locked", "--offline", "--format-version", "1",
         "--filter-platform", args.target],
        repo,
    )
    metadata = json.loads(metadata_json)
    packages = dependency_packages(metadata)
    root_package = next(package for package in metadata["packages"] if package["name"] == "ccl-cli")
    toolchain_cargo = command_output(["cargo", "--version"], repo)
    toolchain_rustc = command_output(["rustc", "--version"], repo)
    lock_path = repo / "Cargo.lock"
    lock_hash = hashlib.sha256(lock_path.read_bytes()).hexdigest()

    licenses_dir = output_dir / "licenses"
    if licenses_dir.exists():
        shutil.rmtree(licenses_dir)
    licenses_dir.mkdir(parents=True, exist_ok=True)
    errors = []
    entries = []
    used_dirs = set()
    for package in packages:
        name = package["name"]
        version = package["version"]
        license_expression = package.get("license")
        if not isinstance(license_expression, str) or not license_expression.strip():
            errors.append(f"{name} {version}: Cargo metadata has no license field")
            continue
        root, files, non_text = license_files(package)
        if not files:
            suffix = f"; non-text license-named files: {', '.join(map(str, non_text))}" if non_text else ""
            errors.append(f"{name} {version}: no UTF-8 license/notice text found under {root}{suffix}")
            continue

        identity = package_identity(package, repo)
        suffix = hashlib.sha256(f"{name}@{version}|{identity}".encode()).hexdigest()[:12]
        slug = re.sub(r"[^A-Za-z0-9._-]+", "-", f"{name}-{version}").strip("-.") or "crate"
        directory = f"{slug}-{suffix}"
        if directory in used_dirs:
            errors.append(f"{name} {version}: duplicate license output directory {directory}")
            continue
        used_dirs.add(directory)
        package_output = licenses_dir / directory
        copied_paths = []
        for file_path in files:
            try:
                relative = file_path.relative_to(root)
            except ValueError:
                relative = Path("declared") / file_path.name
            destination = package_output / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(file_path, destination)
            copied_paths.append(destination.relative_to(output_dir).as_posix())

        entries.append({
            "name": name,
            "version": version,
            "license": license_expression.strip(),
            "source_url": source_url(package, repo),
            "files": sorted(copied_paths),
        })

    if errors:
        raise RuntimeError("third-party notice generation failed:\n- " + "\n- ".join(errors))

    output_dir.mkdir(parents=True, exist_ok=True)
    for language in ("en", "zh-CN"):
        output = output_dir / f"THIRD-PARTY.{language}.md"
        with output.open("w", encoding="utf-8", newline="\n") as destination:
            destination.write(render_inventory(
                language, args.target, root_package, toolchain_cargo,
                toolchain_rustc, lock_hash, entries,
            ))
    print(f"Bundled notices for {len(entries)} third-party crates ({args.target}).")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(error, file=sys.stderr)
        raise SystemExit(1)
