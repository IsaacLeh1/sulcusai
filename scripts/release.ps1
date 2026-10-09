# SPDX-License-Identifier: AGPL-3.0-only
# Builds a signed SulcusAI installer and the latest.json the app's updater
# reads. With -Publish it also creates the GitHub release vX.Y.Z (on the
# public IsaacLeh1/sulcusai repo) with both files attached.
#
#   powershell -File scripts\release.ps1                  # build and sign only
#   powershell -File scripts\release.ps1 -Publish -Notes "What's new"
#
# The signing key lives at %USERPROFILE%\.tauri\sulcusai.key (never commit
# it; without it, installed copies can never update again).

param(
    [switch]$Publish,
    [string]$Notes = "",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$keyFile = Join-Path $env:USERPROFILE ".tauri\sulcusai.key"
if (-not (Test-Path $keyFile)) { throw "Signing key not found at $keyFile" }

$version = (Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
$tag = "v$version"
$bundle = Join-Path $root "src-tauri\target\release\bundle\nsis"
$exeName = "SulcusAI_${version}_x64-setup.exe"
$exe = Join-Path $bundle $exeName
$sig = "$exe.sig"

if (-not $SkipBuild) {
    $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $keyFile -Raw
    $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
    Push-Location $root
    try {
        pnpm tauri build
        if ($LASTEXITCODE -ne 0) { throw "The build failed." }
    } finally {
        Pop-Location
        Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY -ErrorAction SilentlyContinue
    }
}
if (-not (Test-Path $sig)) { throw "No signature next to $exeName; was it built with the signing key?" }

$latest = [ordered]@{
    version   = $version
    notes     = $Notes
    pub_date  = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = [ordered]@{
        "windows-x86_64" = [ordered]@{
            signature = (Get-Content $sig -Raw).Trim()
            url       = "https://github.com/IsaacLeh1/sulcusai/releases/download/$tag/$exeName"
        }
    }
}
$latestPath = Join-Path $bundle "latest.json"
# UTF-8 without a byte-order mark, which the updater's JSON parser needs.
[System.IO.File]::WriteAllText($latestPath, ($latest | ConvertTo-Json -Depth 5), (New-Object System.Text.UTF8Encoding $false))
Write-Host "Built and signed $exeName; wrote latest.json"

if ($Publish) {
    $title = "SulcusAI $version"
    $body = if ($Notes) { $Notes } else { "SulcusAI $version" }
    gh release create $tag $exe $latestPath --repo IsaacLeh1/sulcusai --target main --title $title --notes $body
    if ($LASTEXITCODE -ne 0) { throw "Publishing the release failed." }
    Write-Host "Published $tag. Installed copies will find it on their next check."
}
