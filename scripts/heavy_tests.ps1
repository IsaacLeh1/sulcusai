# SPDX-License-Identifier: AGPL-3.0-only
# Runs the heavy tests (marked #[ignore]: real models, the graphics card,
# the network) one at a time, resting between them and waiting for the
# graphics card to cool, and writes what passed and failed to a results
# file, with each test's output beside it.
#
#   powershell -File scripts\heavy_tests.ps1 -Filter e2e::models
#   powershell -File scripts\heavy_tests.ps1 -Filter "docs,backup" -Out C:\somewhere\heavy.md
#   powershell -File scripts\heavy_tests.ps1 -List
#
# Tests listed in heavy_tests_hands_on.txt use the screen, the microphone,
# a browser, real accounts, paid cloud models or a relay: they run only
# when named in -Include.

param(
    [string]$Filter = "",
    [string[]]$Include = @(),
    [int]$Rest = 30,
    [int]$GpuMax = 75,
    [int]$MinFreeGb = 10,
    [int]$TestMinutes = 60,
    [string]$Out = (Join-Path $env:TEMP "sulcusai-heavy-$(Get-Date -Format yyyyMMdd-HHmm).md"),
    [switch]$List
)

$root = Split-Path -Parent $PSScriptRoot
# A full-speed build heats the PC as much as the tests do.
if (-not $env:CARGO_BUILD_JOBS) { $env:CARGO_BUILD_JOBS = "6" }

function GpuTemp {
    try { [int](& nvidia-smi --query-gpu=temperature.gpu --format=csv,noheader,nounits 2>$null | Select-Object -First 1) } catch { $null }
}

function FreeGb {
    $free = @{}
    Get-CimInstance Win32_LogicalDisk -Filter "DriveType=3" | ForEach-Object { $free[$_.DeviceID] = [math]::Round($_.FreeSpace / 1GB, 1) }
    $free
}

$handsOn = Get-Content (Join-Path $PSScriptRoot "heavy_tests_hands_on.txt") | ForEach-Object { $_.Trim() } | Where-Object { $_ -and -not $_.StartsWith("#") }
function IsHandsOn($name) {
    # A bare test name, or a module path ending in "::" for all its tests.
    foreach ($h in $handsOn) {
        if ($name -eq $h -or $name.EndsWith("::$h") -or ($h.EndsWith("::") -and ($name.StartsWith($h) -or $name.Contains("::$h")))) { return $true }
    }
    $false
}

Push-Location (Join-Path $root "src-tauri")
try {
    cargo test --lib --no-run
    if ($LASTEXITCODE -ne 0) { throw "The tests don't build." }
    $all = cargo test --lib -- --ignored --list --format terse 2>$null | Where-Object { $_ -match ': test$' } | ForEach-Object { $_ -replace ': test$', '' }
    $filters = $Filter -split ','
    $picked = @($all | Where-Object { $n = $_; @($filters | Where-Object { $n -like "*$($_.Trim())*" }).Count -gt 0 })
    $run = @($picked | Where-Object { -not (IsHandsOn $_) -or $Include -contains $_ })
    $held = @($picked | Where-Object { (IsHandsOn $_) -and $Include -notcontains $_ })
    if ($List) {
        $run | ForEach-Object { "run   $_" }
        $held | ForEach-Object { "held  $_" }
        return
    }
    $logs = [System.IO.Path]::ChangeExtension($Out, $null).TrimEnd('.') + "-logs"
    New-Item -ItemType Directory -Force $logs | Out-Null
    function Report {
        $passed = @($rows | Where-Object { $_.Result -eq "passed" }).Count
        $lines = @(
            "# Heavy tests, $(Get-Date -Format 'yyyy-MM-dd HH:mm')",
            "",
            "Commit $(git log --oneline -1)$(if (git status --porcelain --untracked-files=no) { ', with changes not yet committed' }). Filter: '$Filter'. $passed of $($rows.Count) passed, of $($run.Count) to run; $($held.Count) held back (hands-on). Each test's output: $logs",
            ""
        )
        if ($stopped) { $lines += @($stopped, "") }
        $lines += @("| Test | Result | Seconds |", "|---|---|---|")
        $lines += $rows | ForEach-Object { "| $($_.Name) | $($_.Result) | $($_.Seconds) |" }
        if ($held.Count) { $lines += @("", "Held back: " + ($held -join ", ")) }
        [System.IO.File]::WriteAllLines($Out, $lines, (New-Object System.Text.UTF8Encoding $false))
    }
    $rows = @()
    $stopped = ""
    # Some tests download models and engines: a drive that runs low stops
    # the run, rather than filling up. Drives already low aren't watched.
    $watch = @((FreeGb).GetEnumerator() | Where-Object { $_.Value -ge $MinFreeGb } | ForEach-Object { $_.Key })
    $i = 0
    foreach ($name in $run) {
        $i++
        if ($i -gt 1) { Start-Sleep -Seconds $Rest }
        $low = (FreeGb).GetEnumerator() | Where-Object { $watch -contains $_.Key -and $_.Value -lt $MinFreeGb } | Select-Object -First 1
        if ($low) {
            $stopped = "Stopped before $name`: $($low.Key) had $($low.Value) GB free (under $MinFreeGb GB)."
            Write-Host $stopped
            break
        }
        while (($t = GpuTemp) -and $t -gt $GpuMax) {
            Write-Host "Graphics card at $t C, waiting for it to cool below $GpuMax C"
            Start-Sleep -Seconds 15
        }
        Write-Host "[$i/$($run.Count)] $name"
        $log = Join-Path $logs (($name -replace '::', '.') + ".log")
        $sw = [Diagnostics.Stopwatch]::StartNew()
        # Run apart, so a test that hangs can be stopped with everything it started.
        $p = Start-Process -FilePath cargo -ArgumentList @("test", "--lib", "--", "--ignored", "--exact", $name, "--test-threads=1") -NoNewWindow -PassThru -RedirectStandardOutput "$log.out" -RedirectStandardError "$log.err"
        $null = $p.Handle
        $hung = -not $p.WaitForExit($TestMinutes * 60000)
        if ($hung) { & taskkill /T /F /PID $p.Id 2>$null | Out-Null; $p.WaitForExit() }
        $ok = -not $hung -and $p.ExitCode -eq 0
        $sw.Stop()
        $text = ((Get-Content "$log.out", "$log.err" -ErrorAction SilentlyContinue) -join "`n")
        Remove-Item "$log.out", "$log.err" -ErrorAction SilentlyContinue
        $text | Set-Content -Encoding UTF8 $log
        # Some tests read a SULCUSAI_... setting for a file or folder to use,
        # or load a model that isn't downloaded on this PC; stopping at once
        # for that says nothing about the code.
        $result = if ($ok) { "passed" } elseif ($hung) { "stopped after $TestMinutes min" } elseif ($text -match "NotPresent") { "needs a setting" } elseif ($text -match "failed to open GGUF file") { "needs a model" } else { "FAILED" }
        $rows += [pscustomobject]@{ Name = $name; Result = $result; Seconds = [math]::Round($sw.Elapsed.TotalSeconds) }
        Write-Host ("    {0} in {1} s" -f $result, [math]::Round($sw.Elapsed.TotalSeconds))
        # Written after every test, so a long run can be followed and a cut-short one still counts.
        Report
    }
    Report
    Write-Host "$(@($rows | Where-Object { $_.Result -eq 'passed' }).Count) of $($rows.Count) passed. Results: $Out"
} finally {
    Pop-Location
}
