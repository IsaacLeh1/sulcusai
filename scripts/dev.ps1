# SPDX-License-Identifier: AGPL-3.0-only
# Runs SulcusAI straight from the code, for trying changes without building,
# publishing or installing anything. Window changes show as soon as a file is
# saved; core (Rust) changes rebuild and restart the app (about a minute).
#
#   powershell -File scripts\dev.ps1            # with your real data
#   powershell -File scripts\dev.ps1 -TestData  # a separate, empty test folder
#
# Microsoft and Google sign-in come from %USERPROFILE%\.tauri\sulcusai-oauth.env,
# as in release.ps1. Stop it with Ctrl+C here.

param([switch]$TestData)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

if ($TestData) {
    # Your real data stays untouched; models need adding in this copy.
    $env:SULCUSAI_DATA_DIR = Join-Path $env:LOCALAPPDATA "app.sulcusai.desktop-dev"
    Write-Host "Using test data in $env:SULCUSAI_DATA_DIR"
} else {
    # Two copies on the same data would get in each other's way. Closing the
    # installed app's window only hides it, so look for the process.
    $installed = Get-Process sulcusai -ErrorAction SilentlyContinue | Where-Object { $_.Path -and -not $_.Path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase) }
    if ($installed) {
        Write-Host "The installed SulcusAI is still running, and both would use the same data."
        Write-Host "Quit it (right-click its icon by the clock > Quit), then run this again. Or use -TestData."
        exit 1
    }
}

$oauthFile = Join-Path $env:USERPROFILE ".tauri\sulcusai-oauth.env"
if (Test-Path $oauthFile) {
    foreach ($line in Get-Content $oauthFile) {
        if ($line -match '^\s*(SULCUSAI_[A-Z_]+)\s*=\s*(.*?)\s*$' -and $Matches[2]) {
            Set-Item -Path "Env:$($Matches[1])" -Value $Matches[2]
        }
    }
} else {
    Write-Warning "No $oauthFile; Microsoft and Google sign-in won't be built in."
}

Push-Location $root
try {
    pnpm tauri dev
} finally {
    Pop-Location
}
