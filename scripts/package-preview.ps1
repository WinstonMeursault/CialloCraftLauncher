$ErrorActionPreference = 'Stop'
$target = $env:TARGET
if (-not $target) { throw 'TARGET is required' }
if ($target -notmatch '^[A-Za-z0-9_-]+$') { throw 'Invalid TARGET' }
$name = "ccl-$target"
$dist = Join-Path (Get-Location) 'dist'
$stage = Join-Path $dist $name
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force (Join-Path $stage 'docs') | Out-Null
Copy-Item "target/$target/release/ccl.exe" (Join-Path $stage 'ccl.exe')
Copy-Item LICENSE, README.md, README.zh-CN.md $stage
python scripts/third-party-notices.py --target $target --output-dir $stage
if ($LASTEXITCODE -ne 0) { throw 'Third-party notice generation failed' }
$documents = @(Get-ChildItem docs -File | Where-Object { $_.Name -match '\.(en|zh-CN)\.md$' })
foreach ($document in $documents) {
    $counterpart = if ($document.Name -match '\.en\.md$') { $document.FullName -replace '\.en\.md$', '.zh-CN.md' } else { $document.FullName -replace '\.zh-CN\.md$', '.en.md' }
    if (-not (Test-Path $counterpart)) { throw "Missing paired document for $($document.Name)" }
    Copy-Item $document.FullName (Join-Path $stage 'docs')
}
$requiredGuides = @('user-guide.en.md', 'user-guide.zh-CN.md', 'api.en.md', 'api.zh-CN.md', 'development.en.md', 'development.zh-CN.md')
foreach ($guide in $requiredGuides) {
    if (-not (Test-Path (Join-Path $stage "docs/$guide"))) { throw "Missing guide: $guide" }
}
if (Test-Path (Join-Path $stage 'licenses')) { throw 'Unexpected separate third-party license files' }
$requiredFiles = @('ccl.exe', 'LICENSE', 'README.md', 'README.zh-CN.md', 'THIRD-PARTY.en.md', 'THIRD-PARTY.zh-CN.md', 'THIRD-PARTY-LICENSES.en.md', 'THIRD-PARTY-LICENSES.zh-CN.md') + @($documents | ForEach-Object { "docs/$($_.Name)" })
& (Join-Path $stage 'ccl.exe') --help | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Packaged ccl --help failed' }
foreach ($required in $requiredFiles) {
  if (-not (Test-Path (Join-Path $stage $required))) { throw "Missing package file: $required" }
}
$archive = Join-Path $dist "$name.zip"
Compress-Archive -Path $stage -DestinationPath $archive -Force
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead($archive)
try {
  $entryCount = $zip.Entries.Count
  $hasSeparateLicenses = @($zip.Entries | Where-Object { $_.FullName -match '(^|/)licenses/' }).Count -gt 0
} finally {
  $zip.Dispose()
}
if ($entryCount -gt 256) { throw "Archive has $entryCount entries; updater limit is 256" }
if ($hasSeparateLicenses) { throw 'Archive contains separate third-party license files' }
$smoke = Join-Path $env:TEMP "$name-archive-smoke"
Remove-Item $smoke -Recurse -Force -ErrorAction SilentlyContinue
Expand-Archive -Path $archive -DestinationPath $smoke -Force
foreach ($required in $requiredFiles) {
  if (-not (Test-Path (Join-Path $smoke "$name/$required"))) { throw "Archive missing: $required" }
}
Remove-Item $smoke -Recurse -Force
$hash = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  $name.zip" | Set-Content -Encoding ascii "$archive.sha256"
