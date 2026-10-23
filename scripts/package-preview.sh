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
python3 scripts/third-party-notices.py --target "$target" --output-dir "$stage"
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
for required in LICENSE README.md README.zh-CN.md docs/user-guide.en.md docs/user-guide.zh-CN.md docs/api.en.md docs/api.zh-CN.md docs/development.en.md docs/development.zh-CN.md; do
  test -s "$stage/$required"
done
third_party_files=(THIRD-PARTY.en.md THIRD-PARTY.zh-CN.md)
while IFS= read -r -d '' file; do
  third_party_files+=("${file#"$stage"/}")
done < <(find "$stage/licenses" -type f -print0)
(( ${#third_party_files[@]} > 2 )) || { echo 'No third-party license texts were generated' >&2; exit 1; }
for required in "${third_party_files[@]}"; do
  test -s "$stage/$required"
done
archive="dist/${name}.${extension}"
# zip updates an existing archive in place, retaining obsolete entries. Start
# fresh so a repeated package contains exactly the current staging directory.
rm -f "$archive"
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
for item in ccl LICENSE README.md README.zh-CN.md "${documents[@]}" "${third_party_files[@]}"; do
  grep -Fx "${name}/${item}" <<<"$entries" >/dev/null
done
if command -v sha256sum >/dev/null 2>&1; then
  (cd dist && sha256sum "${name}.${extension}" > "${name}.${extension}.sha256")
else
  (cd dist && shasum -a 256 "${name}.${extension}" > "${name}.${extension}.sha256")
fi
