# SPDX-License-Identifier: AGPL-3.0-only
# How big SulcusAI is to download and once installed: the installer, the
# program, the window's files, and how many Rust crates go into it.
#
#   powershell -File scripts\measure_size.ps1

$root =Split-Path -Parent $PSScriptRoot
$version = (Get-Content (Join-Path $root "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json).version
$installer = Join-Path $root "src-tauri\target\release\bundle\nsis\SulcusAI_${version}_x64-setup.exe"
$exe = Join-Path $root "src-tauri\target\release\sulcusai.exe"
$dist = Join-Path $root "dist"
function Mb($bytes) { "{0:N1} MB" -f ($bytes / 1MB) }

if (Test-Path $installer) { "Installer ${version}: $(Mb (Get-Item $installer).Length)" } else { "No installer for $version yet (scripts\release.ps1 builds it)." }
if (Test-Path $exe) { "Program: $(Mb (Get-Item $exe).Length), built $((Get-Item $exe).LastWriteTime)" }
if (Test-Path $dist) { "Window files: $(Mb ((Get-ChildItem $dist -Recurse -File | Measure-Object Length -Sum).Sum))" }
Push-Location (Join-Path $root "src-tauri")
try {
    $crates = cargo tree --prefix none -e normal --target x86_64-pc-windows-msvc 2>$null | ForEach-Object { ($_ -split ' ')[0..1] -join ' ' } | Sort-Object -Unique
    "Rust crates in the Windows program: $(@($crates).Count)"
} finally { Pop-Location }
# Downloaded only when a feature needs them, so not part of the download.
$engines = Join-Path $env:LOCALAPPDATA "app.sulcusai.desktop\engines"
if (Test-Path $engines) {
    "Engines downloaded on this PC: $(Mb ((Get-ChildItem $engines -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum))"
}
