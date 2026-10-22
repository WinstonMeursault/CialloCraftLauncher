$ErrorActionPreference = 'Stop'
$target = $env:TARGET
if (-not $target) { throw 'TARGET is required' }
$name = "ccl-$target"
$dist = Join-Path (Get-Location) 'dist'
$stage = Join-Path $dist $name
New-Item -ItemType Directory -Force (Join-Path $stage 'docs') | Out-Null
Copy-Item "target/$target/release/ccl.exe" (Join-Path $stage 'ccl.exe')
Copy-Item LICENSE, README.md, README.zh-CN.md $stage
Copy-Item docs/cli.en.md, docs/cli.zh-CN.md, docs/architecture.en.md, docs/architecture.zh-CN.md, docs/development.en.md, docs/development.zh-CN.md, docs/accounts.en.md, docs/accounts.zh-CN.md (Join-Path $stage 'docs')
& (Join-Path $stage 'ccl.exe') --help | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'Packaged ccl --help failed' }
foreach ($required in @('LICENSE', 'README.md', 'README.zh-CN.md', 'docs/cli.en.md', 'docs/cli.zh-CN.md', 'docs/architecture.en.md', 'docs/architecture.zh-CN.md', 'docs/development.en.md', 'docs/development.zh-CN.md', 'docs/accounts.en.md', 'docs/accounts.zh-CN.md')) {
  if (-not (Test-Path (Join-Path $stage $required))) { throw "Missing package file: $required" }
}
$archive = Join-Path $dist "$name.zip"
Compress-Archive -Path $stage -DestinationPath $archive -Force
$smoke = Join-Path $env:TEMP "$name-archive-smoke"
Remove-Item $smoke -Recurse -Force -ErrorAction SilentlyContinue
Expand-Archive -Path $archive -DestinationPath $smoke -Force
foreach ($required in @('ccl.exe', 'LICENSE', 'README.md', 'README.zh-CN.md', 'docs/cli.en.md', 'docs/cli.zh-CN.md', 'docs/architecture.en.md', 'docs/architecture.zh-CN.md', 'docs/development.en.md', 'docs/development.zh-CN.md', 'docs/accounts.en.md', 'docs/accounts.zh-CN.md')) {
  if (-not (Test-Path (Join-Path $smoke "$name/$required"))) { throw "Archive missing: $required" }
}
Remove-Item $smoke -Recurse -Force
$hash = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  $name.zip" | Set-Content -Encoding ascii "$archive.sha256"
