$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$binary = Join-Path $root "dist/not-news-windows-x86_64.exe"
$expectedCommit = (git rev-parse HEAD).Trim().Substring(0, 12)

if (-not (Test-Path $binary -PathType Leaf)) { throw "missing standalone Windows executable" }
if ((Get-ChildItem dist -File).Count -ne 1) { throw "Windows dist must contain exactly one file" }
$header = [IO.File]::ReadAllBytes($binary)[0..1]
if ($header[0] -ne 0x4d -or $header[1] -ne 0x5a) { throw "release is not a Windows PE executable" }

$scratch = Join-Path $env:RUNNER_TEMP "not-news-single-executable-check"
Remove-Item $scratch -Recurse -Force -ErrorAction SilentlyContinue
New-Item $scratch -ItemType Directory | Out-Null
$relocated = Join-Path $scratch "renamed-not-news.exe"
Copy-Item $binary $relocated
if ((Get-FileHash $binary).Hash -ne (Get-FileHash $relocated).Hash) { throw "relocation changed the executable" }

function Invoke-SelfCheck([string]$renderer, [string]$name) {
    $previous = $env:NOT_NEWS_FORCE_SOFTWARE
    if ($renderer -eq "skia-raster") { $env:NOT_NEWS_FORCE_SOFTWARE = "1" } else { Remove-Item Env:NOT_NEWS_FORCE_SOFTWARE -ErrorAction SilentlyContinue }
    try {
        $output = & $relocated --release-self-check (Join-Path $scratch $name)
        if ($LASTEXITCODE -ne 0) { throw "release self-check failed" }
        $report = $output | ConvertFrom-Json
        if ($report.release_self_check -ne "pass" -or -not $report.empty_launch) { throw "release self-check contract failed" }
        if ($report.commit -ne $expectedCommit) { throw "release commit identity is stale" }
        if ($renderer -eq "skia-raster" -and $report.renderer -ne "skia-raster") { throw "software renderer was not exercised" }
    } finally {
        $env:NOT_NEWS_FORCE_SOFTWARE = $previous
    }
}

Invoke-SelfCheck "auto" "automatic"
Invoke-SelfCheck "skia-raster" "software"
$capabilities = & $relocated --capability-check (Join-Path $scratch "capabilities") | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $capabilities.capability_check -ne "pass") { throw "capability check failed" }
if ($capabilities.commit -ne $expectedCommit) { throw "capability check came from a stale executable" }
