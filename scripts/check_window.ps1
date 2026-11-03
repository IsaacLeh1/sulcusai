# SPDX-License-Identifier: AGPL-3.0-only
# Checks that a built SulcusAI draws its window (0.9.49 went out with a
# blank one). The program runs hidden on a scratch data folder, so the copy
# in use and its data are left alone; see src-tauri/src/selftest.rs.
#
#   powershell -File scripts\check_window.ps1            # target\release\sulcusai.exe
#   powershell -File scripts\check_window.ps1 -Exe <path>

param(
    [string]$Exe = (Join-Path (Split-Path -Parent $PSScriptRoot) "src-tauri\target\release\sulcusai.exe")
)

$ErrorActionPreference = "Stop"
if (-not (Test-Path $Exe)) { throw "No program at $Exe; build it first." }
$work = Join-Path $env:TEMP "sulcusai-window-check-$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$result = Join-Path $work "result.txt"
$env:SULCUSAI_DATA_DIR = Join-Path $work "data"
$env:SULCUSAI_SELFTEST = $result
try {
    $p = Start-Process -FilePath $Exe -PassThru
    # Without reading the handle now, ExitCode stays empty after it exits.
    $null = $p.Handle
    # The program gives up by itself after 90 seconds.
    if (-not $p.WaitForExit(150000)) {
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
        throw "The built program neither drew its window nor gave up in 150 seconds."
    }
    $said = if (Test-Path $result) { [System.IO.File]::ReadAllText($result).Trim() } else { "" }
    if ($p.ExitCode -ne 0 -or -not $said.StartsWith("drawn")) {
        throw "The built window didn't draw (exit code $($p.ExitCode)): $said"
    }
    Write-Host "The built window draws."
} finally {
    Remove-Item Env:SULCUSAI_SELFTEST, Env:SULCUSAI_DATA_DIR -ErrorAction SilentlyContinue
    # Its helper processes end with it; a moment for them to let go of files.
    Start-Sleep -Seconds 2
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
