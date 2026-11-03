# SPDX-License-Identifier: AGPL-3.0-only
# What a running SulcusAI costs while left alone: processor time, memory,
# disk writes, graphics memory and open connections of its whole process
# tree (WebView2 and engines included), sampled $Seconds apart.
#
#   powershell -File scripts\measure_idle.ps1 -Seconds 90 -Label "in the tray"

param([int]$Seconds = 90, [string]$Label = "idle")

function Tree {
    $main = Get-Process sulcusai -ErrorAction SilentlyContinue
    if (-not $main) { throw "SulcusAI isn't running." }
    $all = Get-CimInstance Win32_Process
    $ids = New-Object System.Collections.Generic.HashSet[int]
    $main | ForEach-Object { [void]$ids.Add($_.Id) }
    do {
        $added = 0
        foreach ($p in $all) {
            if ($ids.Contains([int]$p.ParentProcessId) -and -not $ids.Contains([int]$p.ProcessId)) { [void]$ids.Add([int]$p.ProcessId); $added++ }
        }
    } while ($added -gt 0)
    $snap = @{}
    foreach ($p in $all) {
        if (-not $ids.Contains([int]$p.ProcessId)) { continue }
        $gp = Get-Process -Id $p.ProcessId -ErrorAction SilentlyContinue
        if ($gp) {
            $snap[[int]$p.ProcessId] = [pscustomobject]@{
                Name = $p.Name; Cpu = $gp.TotalProcessorTime.TotalSeconds; WS = $gp.WorkingSet64; Priv = $gp.PrivateMemorySize64
                WriteOps = [int64]$p.WriteOperationCount; WriteBytes = [int64]$p.WriteTransferCount; Threads = $gp.Threads.Count
            }
        }
    }
    $snap
}

$a = Tree; $t0 = Get-Date
Start-Sleep -Seconds $Seconds
$b = Tree; $dt = ((Get-Date) - $t0).TotalSeconds
$cores = [Environment]::ProcessorCount
"== $Label, $([math]::Round($dt)) s, $cores logical processors"
$tot = [pscustomobject]@{ Cpu = 0.0; WS = 0; Priv = 0; WriteOps = 0; WriteBytes = 0 }
foreach ($id in $b.Keys) {
    $x = $b[$id]; $y = $a[$id]
    # A process started between the samples counts from its start.
    $cpu = if ($y) { $x.Cpu - $y.Cpu } else { $x.Cpu }
    $wo = if ($y) { $x.WriteOps - $y.WriteOps } else { 0 }
    $wb = if ($y) { $x.WriteBytes - $y.WriteBytes } else { 0 }
    "{0,6} {1,-20} cpu {2,6:N3}% of one core  WS {3,7:N1} MB  private {4,7:N1} MB  disk writes {5,4} ({6,8:N1} KB)  threads {7}" -f $id, $x.Name, (100 * $cpu / $dt), ($x.WS / 1MB), ($x.Priv / 1MB), $wo, ($wb / 1KB), $x.Threads
    $tot.Cpu += $cpu; $tot.WS += $x.WS; $tot.Priv += $x.Priv; $tot.WriteOps += $wo; $tot.WriteBytes += $wb
}
"TOTAL  cpu {0:N3}% of one core ({1:N4}% of the whole processor)  WS {2:N1} MB  private {3:N1} MB  disk writes {4} ({5:N1} KB)" -f (100 * $tot.Cpu / $dt), (100 * $tot.Cpu / $dt / $cores), ($tot.WS / 1MB), ($tot.Priv / 1MB), $tot.WriteOps, ($tot.WriteBytes / 1KB)
try {
    $gpu = (Get-Counter '\GPU Process Memory(*)\Dedicated Usage' -ErrorAction Stop).CounterSamples
    $g = 0; foreach ($s in $gpu) { if ($s.InstanceName -match 'pid_(\d+)_' -and $b.ContainsKey([int]$Matches[1])) { $g += $s.CookedValue } }
    "Graphics memory: {0:N1} MB" -f ($g / 1MB)
    $eng = (Get-Counter '\GPU Engine(*)\Utilization Percentage' -ErrorAction Stop).CounterSamples
    $u = 0; foreach ($s in $eng) { if ($s.InstanceName -match 'pid_(\d+)_' -and $b.ContainsKey([int]$Matches[1])) { $u += $s.CookedValue } }
    "Graphics engines busy (sum, at this moment): {0:N2}%" -f $u
} catch { "Graphics counters unavailable: $($_.Exception.Message)" }
$conns = Get-NetTCPConnection -State Established -ErrorAction SilentlyContinue | Where-Object { $b.ContainsKey([int]$_.OwningProcess) }
"Open network connections: $(@($conns).Count)"
$conns | Group-Object RemoteAddress | ForEach-Object { "  $($_.Name) x$($_.Count)" }
