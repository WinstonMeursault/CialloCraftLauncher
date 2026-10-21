#!/usr/bin/env bash
set -euo pipefail

target="${TARGET:?TARGET is required}"
extension="${ARCHIVE_EXT:?ARCHIVE_EXT is required}"
name="ccl-${target}"
stage="dist/${name}"
mkdir -p "$stage/docs"
install -m 755 "target/${target}/release/ccl" "$stage/ccl"
cp LICENSE README.md README.zh-CN.md "$stage/"
cp docs/cli.en.md docs/cli.zh-CN.md docs/architecture.en.md docs/architecture.zh-CN.md docs/development.en.md docs/development.zh-CN.md docs/accounts.en.md docs/accounts.zh-CN.md "$stage/docs/"
"$stage/ccl" --help >/dev/null
test -s "$stage/LICENSE" && test -s "$stage/README.md" && test -s "$stage/docs/cli.en.md" && test -s "$stage/docs/cli.zh-CN.md" && test -s "$stage/docs/development.en.md" && test -s "$stage/docs/development.zh-CN.md" && test -s "$stage/docs/accounts.en.md" && test -s "$stage/docs/accounts.zh-CN.md"
archive="dist/${name}.${extension}"
if [[ "$extension" == zip ]]; then
  (cd dist && zip -qr "${name}.zip" "$name")
else
  tar -C dist -czf "$archive" "$name"
fi
if [[ "$extension" == zip ]]; then
  entries="$(unzip -Z1 "$archive")"
else
  entries="$(tar -tzf "$archive")"
fi
for item in ccl LICENSE README.md README.zh-CN.md docs/cli.en.md docs/cli.zh-CN.md docs/architecture.en.md docs/architecture.zh-CN.md docs/development.en.md docs/development.zh-CN.md docs/accounts.en.md docs/accounts.zh-CN.md; do
  grep -Fx "${name}/${item}" <<<"$entries" >/dev/null
done
if command -v sha256sum >/dev/null 2>&1; then
  (cd dist && sha256sum "${name}.${extension}" > "${name}.${extension}.sha256")
else
  (cd dist && shasum -a 256 "${name}.${extension}" > "${name}.${extension}.sha256")
fi
