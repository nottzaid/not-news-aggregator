$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not (Get-Command cargo-packager -ErrorAction SilentlyContinue)) {
    throw "cargo-packager 0.11.8 is required"
}

$metadata = cargo metadata --filter-platform x86_64-pc-windows-msvc --format-version 1 | ConvertFrom-Json
$version = ($metadata.packages | Where-Object name -eq "not-news-app").version
$commit = if ($env:GITHUB_SHA) { $env:GITHUB_SHA } else { (git rev-parse HEAD).Trim() }
$env:NOT_NEWS_BUILD_COMMIT = $commit.Substring(0, 12)

Remove-Item dist -Recurse -Force -ErrorAction SilentlyContinue
cargo build --locked --release -p not-news-app
if ($LASTEXITCODE -ne 0) { throw "release build failed" }
cargo-packager --config Packager.toml --formats nsis
if ($LASTEXITCODE -ne 0) { throw "NSIS packaging failed" }

$generated = Get-ChildItem dist -File -Filter "*.exe" | Select-Object -First 1
if (-not $generated) { throw "cargo-packager did not produce an NSIS installer" }
$installer = Join-Path $root "dist/not-news_${version}_windows-x86_64-setup.exe"
Move-Item $generated.FullName $installer

$dependencies = [ordered]@{
    schema = 1
    target = "x86_64-pc-windows-msvc"
    packages = @(
        $resolved = @{}
        foreach ($node in $metadata.resolve.nodes) { $resolved[$node.id] = $true }
        $metadata.packages |
            Where-Object { $null -ne $_.source -and $resolved.ContainsKey($_.id) } |
            Sort-Object name, version |
            ForEach-Object {
                [ordered]@{
                    name = $_.name
                    version = $_.version
                    license = $_.license
                    source = $_.source
                }
            }
    )
}
$utf8 = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText(
    (Join-Path $root "dist/DEPENDENCIES-windows-x86_64.json"),
    ($dependencies | ConvertTo-Json -Depth 5),
    $utf8
)
$buildInfo = [ordered]@{
    schema = 1
    product = "Not News"
    version = $version
    commit = $commit
    target = "x86_64-pc-windows-msvc"
    rustc = (rustc --version)
    packager = (cargo-packager --version)
}
[IO.File]::WriteAllText(
    (Join-Path $root "dist/BUILDINFO-windows-x86_64.json"),
    ($buildInfo | ConvertTo-Json -Depth 3),
    $utf8
)

$portableRoot = Join-Path $env:RUNNER_TEMP "not-news-portable"
Remove-Item $portableRoot -Recurse -Force -ErrorAction SilentlyContinue
$portable = Join-Path $portableRoot "not-news_${version}_windows-x86_64"
New-Item $portable -ItemType Directory -Force | Out-Null
Copy-Item -Path @(
    "target/release/not-news-app.exe",
    "README.md",
    "dist/BUILDINFO-windows-x86_64.json",
    "dist/DEPENDENCIES-windows-x86_64.json"
) -Destination $portable
$zip = Join-Path $root "dist/not-news_${version}_windows-x86_64.zip"
Compress-Archive -Path $portable -DestinationPath $zip -CompressionLevel Optimal

$artifacts = Get-ChildItem dist -File | Sort-Object Name
$lines = foreach ($artifact in $artifacts) {
    $hash = (Get-FileHash $artifact.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $($artifact.Name)"
}
[IO.File]::WriteAllLines(
    (Join-Path $root "dist/SHA256SUMS-windows-x86_64.txt"),
    $lines,
    $utf8
)
