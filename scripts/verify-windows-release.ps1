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
$runtimeFile = Join-Path $root "dist/RUNTIME-windows-x86_64.json"
Remove-Item $runtimeFile -Force -ErrorAction SilentlyContinue
$baseHashes = [IO.File]::ReadAllLines($hashFile) | Where-Object { $_ -notmatch '  RUNTIME-windows-x86_64\.json$' }
[IO.File]::WriteAllLines($hashFile, $baseHashes, [Text.UTF8Encoding]::new($false))
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

function Invoke-ReleaseSelfCheck([string]$Binary, [string]$CheckRoot, [string]$Renderer) {
    $output = (& $Binary --release-self-check $CheckRoot | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw "$Binary self-check failed" }
    Write-Host $output
    $result = $output | ConvertFrom-Json
    if ($result.release_self_check -ne "pass") { throw "$Binary did not report a passing self-check" }
    if ($result.empty_launch -ne $true) { throw "$Binary did not present an empty first launch" }
    if ($result.commit -ne $buildInfo.commit.Substring(0, 12)) { throw "$Binary reported the wrong commit" }
    if ($Renderer -eq "auto") {
        if ($result.renderer -notin @("skia-opengl", "skia-raster")) { throw "$Binary reported unknown renderer $($result.renderer)" }
    } elseif ($result.renderer -ne $Renderer) {
        throw "$Binary selected $($result.renderer), expected $Renderer"
    }
    return $result
}

$zip = Join-Path $root "dist/not-news_${version}_windows-x86_64.zip"
Expand-Archive $zip (Join-Path $scratch "portable")
$portable = Join-Path $scratch "portable/not-news_${version}_windows-x86_64/not-news-app.exe"
if ((Get-FileHash "assets/fonts/manrope/OFL.txt").Hash -ne (Get-FileHash (Join-Path (Split-Path $portable) "licenses/Manrope-OFL.txt")).Hash) { throw "portable archive changed the Manrope license" }
if ((Get-FileHash "assets/fonts/jetbrainsmono/OFL.txt").Hash -ne (Get-FileHash (Join-Path (Split-Path $portable) "licenses/JetBrains-Mono-OFL.txt")).Hash) { throw "portable archive changed the JetBrains Mono license" }
foreach ($guide in @("README.md", "HERMES.md", "OPERATING.md")) {
    if ((Get-FileHash $guide).Hash -ne (Get-FileHash (Join-Path (Split-Path $portable) $guide)).Hash) { throw "portable archive changed or omitted $guide" }
}
$portableAuto = Invoke-ReleaseSelfCheck $portable (Join-Path $scratch "portable-check") "auto"
$ignoredConfiguration = @("EXA_API_KEY", "GROQ_API_KEY", "BROWSERBASE_API_KEY", "SEARXNG_URL")
$previousConfiguration = @{}
foreach ($name in $ignoredConfiguration) {
    $previousConfiguration[$name] = [Environment]::GetEnvironmentVariable($name, "Process")
    [Environment]::SetEnvironmentVariable($name, "must-be-ignored", "Process")
}
try {
    $capabilityOutput = (& $portable --capability-check (Join-Path $scratch "capabilities") | Out-String).Trim()
} finally {
    foreach ($name in $ignoredConfiguration) {
        [Environment]::SetEnvironmentVariable($name, $previousConfiguration[$name], "Process")
    }
}
if ($LASTEXITCODE -ne 0) { throw "portable capability check failed" }
Write-Host $capabilityOutput
$portableCapabilities = $capabilityOutput | ConvertFrom-Json
if ($portableCapabilities.capability_check -ne "pass") { throw "portable capability diagnosis did not pass" }
if ($portableCapabilities.commit -ne $buildInfo.commit.Substring(0, 12)) { throw "portable capability diagnosis reported the wrong commit" }
if ($portableCapabilities.research.runtime -ne "hermes") { throw "Hermes is not the sole research runtime" }
if ($portableCapabilities.research.profile_policy -ne "installed-v2") { throw "Hermes profile policy v2 was not installed" }
if ($portableCapabilities.research.executable -notin @("present", "missing")) { throw "Hermes executable diagnosis is absent" }
if ($portableCapabilities.research.compatibility -ne "deferred-to-exact-pre-research-acp-check") { throw "Hermes compatibility scope is misstated" }
if ($portableCapabilities.discovery.exa_configuration -ne "deferred-os-vault-probe") { throw "Exa environment leaked into configuration" }
if ($portableCapabilities.discovery.searxng_endpoint -ne "missing") { throw "SearXNG environment leaked into configuration" }
if ($portableCapabilities.discovery.browse_executable -notin @("present", "missing")) { throw "Browse executable diagnosis is absent" }
if ($portableCapabilities.discovery.curl_executable -notin @("present", "missing")) { throw "curl executable diagnosis is absent" }
if ($portableCapabilities.discovery.browserbase_configuration -ne "optional-deferred-os-vault-probe") { throw "Browserbase environment leaked into configuration" }
if ($portableCapabilities.transcription.configuration -ne "deferred-os-vault-probe") { throw "Groq environment leaked into configuration" }
if ($portableCapabilities.research.PSObject.Properties.Name -contains "selected") { throw "retired backend selector survived" }
if ($portableCapabilities.research.PSObject.Properties.Name -contains "opencode") { throw "OpenCode leaked into application capabilities" }
$profileRoot = Join-Path $scratch "capabilities\hermes\profiles\not-news"
if (-not (Test-Path (Join-Path $profileRoot "config.yaml"))) { throw "Hermes profile config was not installed" }
if (-not (Test-Path (Join-Path $profileRoot "memories\USER.md"))) { throw "Hermes profile memory policy was not installed" }
if (-not ((Get-Content (Join-Path $profileRoot "SOUL.md") -Raw).Contains("research agent for Not News"))) { throw "Hermes profile identity is wrong" }
if (-not $portableCapabilities.transcription.os_vault_probe) { throw "transcription remediation is absent" }
if (-not $portableCapabilities.microphone.state) { throw "microphone capability is absent" }
if (-not $portableCapabilities.kokoro.state) { throw "Kokoro capability is absent" }
$previousForceSoftware = $env:NOT_NEWS_FORCE_SOFTWARE
$env:NOT_NEWS_FORCE_SOFTWARE = "1"
try {
    $portableSoftware = Invoke-ReleaseSelfCheck $portable (Join-Path $scratch "portable-software-check") "skia-raster"
} finally {
    $env:NOT_NEWS_FORCE_SOFTWARE = $previousForceSoftware
}
$sourceHash = (Get-FileHash "target/release/not-news-app.exe" -Algorithm SHA256).Hash
$portableHash = (Get-FileHash $portable -Algorithm SHA256).Hash
if ($sourceHash -ne $portableHash) { throw "portable archive changed the release executable" }

$installedAuto = $null
$reinstalledAuto = $null

if ($InstallLifecycle) {

$data = Join-Path $env:LOCALAPPDATA "not-news-canvas"
$marker = Join-Path $data "release-preservation-marker"
New-Item $data -ItemType Directory -Force | Out-Null
[IO.File]::WriteAllText($marker, "must survive uninstall")
$installer = Join-Path $root "dist/not-news_${version}_windows-x86_64-setup.exe"
$install = Start-Process $installer -ArgumentList "/S" -PassThru -Wait
if ($install.ExitCode -ne 0) { throw "NSIS installation exited $($install.ExitCode)" }

$installed = Join-Path $env:LOCALAPPDATA "Not News/not-news-app.exe"
if (-not (Test-Path $installed)) { throw "NSIS did not install the executable" }
if (-not (Test-Path (Join-Path $env:LOCALAPPDATA "Not News/licenses/Manrope-OFL.txt"))) { throw "NSIS omitted the Manrope license" }
if (-not (Test-Path (Join-Path $env:LOCALAPPDATA "Not News/licenses/JetBrains-Mono-OFL.txt"))) { throw "NSIS omitted the JetBrains Mono license" }
foreach ($guide in @("README.md", "HERMES.md", "OPERATING.md")) {
    if ((Get-FileHash $guide).Hash -ne (Get-FileHash (Join-Path $env:LOCALAPPDATA "Not News/$guide")).Hash) { throw "NSIS changed or omitted $guide" }
}
if ((Get-FileHash $installed).Hash -ne $sourceHash) { throw "NSIS changed the release executable" }
$installedAuto = Invoke-ReleaseSelfCheck $installed (Join-Path $scratch "installed-check") "auto"

$uninstaller = Join-Path $env:LOCALAPPDATA "Not News/uninstall.exe"
$uninstall = Start-Process $uninstaller -ArgumentList "/S" -PassThru -Wait
if ($uninstall.ExitCode -ne 0) { throw "NSIS uninstallation exited $($uninstall.ExitCode)" }
for ($attempt = 0; $attempt -lt 30 -and (Test-Path $installed); $attempt++) {
    Start-Sleep -Seconds 1
}
if (Test-Path $installed) { throw "NSIS left its executable installed" }
if (-not (Test-Path $marker)) { throw "NSIS uninstaller deleted user research state" }
$reinstall = Start-Process $installer -ArgumentList "/S" -PassThru -Wait
if ($reinstall.ExitCode -ne 0) { throw "NSIS reinstallation exited $($reinstall.ExitCode)" }
if (-not (Test-Path $marker)) { throw "NSIS reinstallation replaced user research state" }
$reinstalledAuto = Invoke-ReleaseSelfCheck $installed (Join-Path $scratch "reinstalled-check") "auto"
$finalUninstall = Start-Process $uninstaller -ArgumentList "/S" -PassThru -Wait
if ($finalUninstall.ExitCode -ne 0) { throw "final NSIS uninstallation exited $($finalUninstall.ExitCode)" }
for ($attempt = 0; $attempt -lt 30 -and (Test-Path $installed); $attempt++) {
    Start-Sleep -Seconds 1
}
if (Test-Path $installed) { throw "final NSIS uninstallation left its executable installed" }
if (-not (Test-Path $marker)) { throw "final NSIS uninstallation deleted user research state" }
Remove-Item $marker
}

$runtime = [ordered]@{
    schema = 1
    commit = $buildInfo.commit
    portable_auto = $portableAuto
    portable_software = $portableSoftware
    portable_capabilities = $portableCapabilities
    installed_auto = $installedAuto
    reinstalled_auto = $reinstalledAuto
}
$utf8 = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText($runtimeFile, ($runtime | ConvertTo-Json -Depth 5), $utf8)
$runtimeHash = (Get-FileHash $runtimeFile -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::AppendAllText(
    $hashFile,
    "$runtimeHash  RUNTIME-windows-x86_64.json`n",
    $utf8
)
