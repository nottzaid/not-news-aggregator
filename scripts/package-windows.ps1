$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$commit = if ($env:GITHUB_SHA) { $env:GITHUB_SHA } else { (git rev-parse HEAD).Trim() }
$env:NOT_NEWS_BUILD_COMMIT = $commit.Substring(0, 12)

Remove-Item dist -Recurse -Force -ErrorAction SilentlyContinue
New-Item dist -ItemType Directory | Out-Null
cargo build --locked --release -p not-news-app
if ($LASTEXITCODE -ne 0) { throw "release build failed" }
Copy-Item target/release/not-news-app.exe dist/not-news-windows-x86_64.exe
