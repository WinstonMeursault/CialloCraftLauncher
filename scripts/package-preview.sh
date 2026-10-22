#!/usr/bin/env bash
set -euo pipefail

target="${TARGET:?TARGET is required}"
extension="${ARCHIVE_EXT:?ARCHIVE_EXT is required}"
[[ "$target" =~ ^[A-Za-z0-9_-]+$ ]] || { echo 'Invalid TARGET' >&2; exit 1; }
name="ccl-${target}"
stage="dist/${name}"
rm -rf "$stage"
mkdir -p "$stage/docs"
install -m 755 "target/${target}/release/ccl" "$stage/ccl"
cp LICENSE README.md README.zh-CN.md "$stage/"
documents=(docs/*.en.md docs/*.zh-CN.md)
for document in "${documents[@]}"; do
  if [[ "$document" == *.en.md ]]; then
    counterpart="${document%.en.md}.zh-CN.md"
  else
    counterpart="${document%.zh-CN.md}.en.md"
  fi
  test -s "$document" && test -s "$counterpart"
  cp "$document" "$stage/docs/"
done
"$stage/ccl" --help >/dev/null
test -s "$stage/LICENSE" && test -s "$stage/README.md" && test -s "$stage/docs/cli.en.md" && test -s "$stage/docs/cli.zh-CN.md" && test -s "$stage/docs/development.en.md" && test -s "$stage/docs/development.zh-CN.md" && test -s "$stage/docs/accounts.en.md" && test -s "$stage/docs/accounts.zh-CN.md" && test -s "$stage/docs/loaders.en.md" && test -s "$stage/docs/loaders.zh-CN.md" && test -s "$stage/docs/modpacks.en.md" && test -s "$stage/docs/modpacks.zh-CN.md"
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
for item in ccl LICENSE README.md README.zh-CN.md "${documents[@]}"; do
  grep -Fx "${name}/${item}" <<<"$entries" >/dev/null
done
if command -v sha256sum >/dev/null 2>&1; then
  (cd dist && sha256sum "${name}.${extension}" > "${name}.${extension}.sha256")
else
  (cd dist && shasum -a 256 "${name}.${extension}" > "${name}.${extension}.sha256")
fi
