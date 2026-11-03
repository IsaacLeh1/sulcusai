# SPDX-License-Identifier: AGPL-3.0-only
# Builds the private connection's Tor helper (sulcusai-tor), zips it for the
# engines release, and prints the catalog entry to paste under "tor" in
# catalog/catalog.json. Uploading the zip is a separate, deliberate step.
#
#   powershell -File scripts\build_tor.ps1 -Build arti-047
#
# The build name goes into the engine folder's name, so it has no dots.

param([Parameter(Mandatory = $true)][string]$Build, [string]$OutDir = "")

if ($Build -match '\.') { throw "Use a build name without dots (the old-engine cleanup reads folder names by their dots)." }
$root = Split-Path -Parent $PSScriptRoot
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root "src-tauri\target" }
if (-not $OutDir) { $OutDir = Join-Path $target "tor-helper" }

Push-Location (Join-Path $root "src-tauri")
try {
    cargo build --release --bin sulcusai-tor --features tor
    if ($LASTEXITCODE -ne 0) { throw "The helper didn't build." }
} finally {
    Pop-Location
}

$exe = Join-Path $target "release\sulcusai-tor.exe"
New-Item -ItemType Directory -Force $OutDir | Out-Null
$zip = Join-Path $OutDir "tor-$Build-cpu-x64.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path $exe -DestinationPath $zip
$size = (Get-Item $zip).Length
$sha = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()

"Helper: {0:N1} MB, zipped {1:N1} MB: {2}" -f ((Get-Item $exe).Length / 1MB), ($size / 1MB), $zip
"Catalog entry (catalog.json, top level):"
[ordered]@{
    tor = [ordered]@{
        name    = "Tor (Arti)"
        license = "MIT OR Apache-2.0"
        build   = $Build
        assets  = [ordered]@{
            "cpu-x64" = [ordered]@{
                url    = "https://github.com/IsaacLeh1/sulcusai/releases/download/engines/tor-$Build-cpu-x64.zip"
                size   = $size
                sha256 = $sha
            }
        }
    }
} | ConvertTo-Json -Depth 5
