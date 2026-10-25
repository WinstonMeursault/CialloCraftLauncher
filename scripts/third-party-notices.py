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
LICENSE_SOURCE_ROOT = Path(__file__).resolve().parent / "license-sources"
LICENSE_SOURCE_MANIFEST = LICENSE_SOURCE_ROOT / "manifest.json"


def pinned_license_sources(package):
    """Return checksum-verified license copies for exact pinned crate sources."""
    if not LICENSE_SOURCE_MANIFEST.is_file():
        return []
    manifest = json.loads(LICENSE_SOURCE_MANIFEST.read_text(encoding="utf-8"))
    key = f"{package['name']}@{package['version']}"
    entry = manifest.get("packages", {}).get(key)
    if not entry:
        return []
    vcs_path = Path(package["manifest_path"]).parent / ".cargo_vcs_info.json"
    try:
        vcs = json.loads(vcs_path.read_text(encoding="utf-8"))
        commit = vcs["git"]["sha1"]
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise RuntimeError(f"{key}: cannot verify pinned upstream source commit: {error}") from error
    if commit != entry["crate_commit"]:
        raise RuntimeError(f"{key}: license fallback commit {entry['crate_commit']} does not match crate source {commit}")
    result = []
    for source in entry["files"]:
        path = LICENSE_SOURCE_ROOT / source["local_path"]
        try:
            contents = path.read_bytes()
        except OSError as error:
            raise RuntimeError(f"{key}: missing tracked license source {path}: {error}") from error
        digest = hashlib.sha256(contents).hexdigest()
        if digest != source["sha256"]:
            raise RuntimeError(f"{key}: license source checksum mismatch for {path}")
        result.append((path, source["source_path"]))
    return result


def command_output(command, cwd):
    result = subprocess.run(command, cwd=cwd, check=False, capture_output=True, text=True, encoding="utf-8")
    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip()
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(command)}\n{detail}")
    return result.stdout.strip()


def dependency_packages(metadata, package_name="ccl-cli"):
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    workspace_members = set(metadata["workspace_members"])
    root = next(
        (package for package in metadata["packages"] if package["name"] == package_name),
        None,
    )
    if root is None or root["id"] not in nodes:
        raise RuntimeError(f"Cargo metadata did not resolve the {package_name} package")

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
    missing_declared = []
    declared = package.get("license_file")
    if declared:
        path = Path(declared)
        if not path.is_absolute():
            path = root / path
        if path.is_file():
            found.add(path)
        else:
            missing_declared.append(path)

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
    upstream_files = [(path, path.relative_to(root).as_posix()) for path in usable]
    if not usable:
        upstream_files = pinned_license_sources(package)
    return root, upstream_files, non_text, missing_declared


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


def inline_code(value):
    runs = [len(match.group(0)) for match in re.finditer(r"`+", value)]
    fence = "`" * (max(runs, default=0) + 1)
    return f"{fence}{value}{fence}"


def render_inventory(language, target, root_package, cargo_version, rustc_version, lock_hash, entries):
    if language == "en":
        bundle_name = "THIRD-PARTY-LICENSES.en.md"
        title = "Third-party notices"
        scope = (
            f"This inventory lists the normal and build-time third-party dependencies "
            f"resolved for `{root_package['name']} {root_package['version']}` on `{target}`. "
            f"Original license and notice texts are included verbatim in `{bundle_name}`."
        )
        metadata = (
            f"Cargo: `{cargo_version}`; Rust: `{rustc_version}`; "
            f"Cargo.lock SHA-256: `{lock_hash}`."
        )
        headers = ("Crate", "Version", "Cargo license", "Source", "Bundled texts")
    else:
        bundle_name = "THIRD-PARTY-LICENSES.zh-CN.md"
        title = "第三方许可证与声明"
        scope = (
            f"此清单列出为 `{root_package['name']} {root_package['version']}` 在 `{target}` "
            f"解析的普通依赖和构建依赖。原始许可证与声明文本完整保存在 `{bundle_name}` 中。"
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
        text_paths = ", ".join(
            f"[{inline_code(item['source_path'])}]({bundle_name}#{item['anchor']})"
            for item in entry["texts"]
        )
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


def render_license_bundle(entries, language):
    if language == "en":
        intro = (
            "# Third-party license and notice texts\n\n"
            "Original source texts are included verbatim and are not translated.\n\n"
        )
        source_label = "Source file: "
    else:
        intro = (
            "# 第三方许可证与声明原文\n\n"
            "以下原始文本从对应 crate 源码中逐字收录，未作翻译。\n\n"
        )
        source_label = "源文件："
    output = bytearray(intro.encode("utf-8"))
    anchors = set()
    for entry in entries:
        output.extend(f"## {entry['name']} {entry['version']}\n\n".encode("utf-8"))
        for item in entry["texts"]:
            anchor = item["anchor"]
            if anchor in anchors:
                raise RuntimeError(f"duplicate license bundle anchor: {anchor}")
            anchors.add(anchor)
            output.extend(f'<a id="{anchor}"></a>\n\n'.encode("utf-8"))
            output.extend(f"### {source_label}{item['source_path']}\n\n".encode("utf-8"))
            text = item["contents"]
            runs = [len(match.group(0)) for match in re.finditer(rb"`+", text)]
            fence = b"`" * (max(3, max(runs, default=0) + 1))
            output.extend(fence + b"text\n")
            output.extend(text)
            if not text.endswith((b"\n", b"\r")):
                output.extend(b"\n")
            output.extend(fence + b"\n\n")
    return bytes(output), anchors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, help="Rust target triple for the packaged executable")
    parser.add_argument("--output-dir", required=True, type=Path, help="Package staging directory")
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--package", default="ccl-cli", choices=("ccl-cli", "ccl-gui"))
    args = parser.parse_args()
    repo = args.repo.resolve()
    output_dir = args.output_dir.resolve()

    metadata_json = command_output(
        ["cargo", "metadata", "--locked", "--offline", "--format-version", "1",
         "--filter-platform", args.target] + (["--features", "ccl-gui/desktop"] if args.package == "ccl-gui" else []),
        repo,
    )
    metadata = json.loads(metadata_json)
    packages = dependency_packages(metadata, args.package)
    root_package = next(package for package in metadata["packages"] if package["name"] == args.package)
    toolchain_cargo = command_output(["cargo", "--version"], repo)
    toolchain_rustc = command_output(["rustc", "--version"], repo)
    lock_path = repo / "Cargo.lock"
    lock_hash = hashlib.sha256(lock_path.read_bytes()).hexdigest()

    legacy_bundle = output_dir / "THIRD-PARTY-LICENSES.md"
    if legacy_bundle.exists():
        legacy_bundle.unlink()
    licenses_dir = output_dir / "licenses"
    if licenses_dir.exists():
        shutil.rmtree(licenses_dir)
    errors = []
    entries = []
    for package in packages:
        name = package["name"]
        version = package["version"]
        license_expression = package.get("license")
        if not isinstance(license_expression, str) or not license_expression.strip():
            errors.append(f"{name} {version}: Cargo metadata has no license field")
            continue
        root, files, non_text, missing_declared = license_files(package)
        if missing_declared:
            errors.append(
                f"{name} {version}: declared license file is missing: "
                + ", ".join(map(str, missing_declared))
            )
            continue
        if non_text:
            errors.append(
                f"{name} {version}: non-UTF-8 license/notice text files: "
                + ", ".join(map(str, non_text))
            )
            continue
        if not files:
            errors.append(f"{name} {version}: no UTF-8 license/notice text found under {root}")
            continue

        identity = package_identity(package, repo)
        source_texts = []
        for file_path, source_path in files:
            anchor = "license-" + hashlib.sha256(
                f"{name}@{version}|{identity}|{source_path}".encode("utf-8")
            ).hexdigest()[:20]
            source_texts.append({
                "source_path": source_path,
                "anchor": anchor,
                "contents": file_path.read_bytes(),
            })

        entries.append({
            "name": name,
            "version": version,
            "license": license_expression.strip(),
            "source_url": source_url(package, repo),
            "texts": source_texts,
        })

    if errors:
        raise RuntimeError("third-party notice generation failed:\n- " + "\n- ".join(errors))

    output_dir.mkdir(parents=True, exist_ok=True)
    for language in ("en", "zh-CN"):
        bundle, anchors = render_license_bundle(entries, language)
        expected_anchors = {
            item["anchor"] for entry in entries for item in entry["texts"]
        }
        if anchors != expected_anchors:
            raise RuntimeError(f"{language} license bundle anchors do not match inventory references")
        bundle_file = output_dir / f"THIRD-PARTY-LICENSES.{language}.md"
        bundle_file.write_bytes(bundle)
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
