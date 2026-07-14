param([switch]$InstallLifecycle)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$metadata = cargo metadata --no-deps --filter-platform x86_64-pc-windows-msvc --format-version 1 | ConvertFrom-Json
$version = ($metadata.packages | Where-Object name -eq "not-news-app").version
$scratch = Join-Path $env:RUNNER_TEMP "not-news-release-check"
Remove-Item $scratch -Recurse -Force -ErrorAction SilentlyContinue
New-Item $scratch -ItemType Directory | Out-Null

$hashFile = Join-Path $root "dist/SHA256SUMS-windows-x86_64.txt"
$buildInfo = Get-Content "dist/BUILDINFO-windows-x86_64.json" -Raw | ConvertFrom-Json
if ($buildInfo.commit -ne (git rev-parse HEAD).Trim()) { throw "build commit does not match checkout" }
if ($buildInfo.packager -ne "cargo-packager 0.11.8") { throw "packager identity does not match" }
$dependencies = Get-Content "dist/DEPENDENCIES-windows-x86_64.json" -Raw | ConvertFrom-Json
if ($dependencies.packages.Count -eq 0) { throw "dependency inventory is empty" }
foreach ($line in [IO.File]::ReadAllLines($hashFile)) {
    if ($line -notmatch '^([0-9a-f]{64})  (.+)$') { throw "malformed hash inventory: $line" }
    $actual = (Get-FileHash (Join-Path $root "dist/$($Matches[2])") -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $Matches[1]) { throw "hash mismatch: $($Matches[2])" }
}

$zip = Join-Path $root "dist/not-news_${version}_windows-x86_64.zip"
Expand-Archive $zip (Join-Path $scratch "portable")
$portable = Join-Path $scratch "portable/not-news_${version}_windows-x86_64/not-news-app.exe"
& $portable --release-self-check (Join-Path $scratch "portable-check")
if ($LASTEXITCODE -ne 0) { throw "portable binary self-check failed" }
$sourceHash = (Get-FileHash "target/release/not-news-app.exe" -Algorithm SHA256).Hash
$portableHash = (Get-FileHash $portable -Algorithm SHA256).Hash
if ($sourceHash -ne $portableHash) { throw "portable archive changed the release executable" }

if (-not $InstallLifecycle) { exit 0 }

$data = Join-Path $env:LOCALAPPDATA "not-news-canvas"
$marker = Join-Path $data "release-preservation-marker"
New-Item $data -ItemType Directory -Force | Out-Null
[IO.File]::WriteAllText($marker, "must survive uninstall")
$installer = Join-Path $root "dist/not-news_${version}_windows-x86_64-setup.exe"
$install = Start-Process $installer -ArgumentList "/S" -PassThru -Wait
if ($install.ExitCode -ne 0) { throw "NSIS installation exited $($install.ExitCode)" }

$installed = Join-Path $env:LOCALAPPDATA "Not News/not-news-app.exe"
if (-not (Test-Path $installed)) { throw "NSIS did not install the executable" }
if ((Get-FileHash $installed).Hash -ne $sourceHash) { throw "NSIS changed the release executable" }
& $installed --release-self-check (Join-Path $scratch "installed-check")
if ($LASTEXITCODE -ne 0) { throw "installed binary self-check failed" }

$uninstaller = Join-Path $env:LOCALAPPDATA "Not News/uninstall.exe"
$uninstall = Start-Process $uninstaller -ArgumentList "/S" -PassThru -Wait
if ($uninstall.ExitCode -ne 0) { throw "NSIS uninstallation exited $($uninstall.ExitCode)" }
for ($attempt = 0; $attempt -lt 30 -and (Test-Path $installed); $attempt++) {
    Start-Sleep -Seconds 1
}
if (Test-Path $installed) { throw "NSIS left its executable installed" }
if (-not (Test-Path $marker)) { throw "NSIS uninstaller deleted user research state" }
Remove-Item $marker
