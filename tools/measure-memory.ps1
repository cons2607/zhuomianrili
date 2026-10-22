# Desktop Calendar - memory measurement (M1 acceptance tool)
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File measure-memory.ps1
# Metrics:
#   TotalWorkingSetMB = main process + app-owned WebView2 processes total working set
#   TaskMgrStyleMB    = sum of per-process private working set (matches Task Manager "Memory" column sum)
# NOTE: keep this file pure ASCII - Windows PowerShell 5.1 reads non-BOM files as ANSI/GBK
#       and Chinese text in comments can corrupt parsing (swallows following lines).

$proc = Get-Process desktop-calendar -ErrorAction SilentlyContinue
if (-not $proc) { Write-Output "APP_NOT_RUNNING"; exit 1 }

$main = $proc | Select-Object -First 1
$childWs = 0
$childCount = 0
$children = Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -match 'com\.desktopcalendar|desktop-calendar' }
foreach ($c in $children) {
    $p = Get-Process -Id $c.ProcessId -ErrorAction SilentlyContinue
    if ($p) { $childWs += $p.WorkingSet64; $childCount++ }
}

$mainWs = $main.WorkingSet64

# Private working set: match by PID (uint32 hash set; values are in bytes)
$targets = New-Object 'System.Collections.Generic.HashSet[uint32]'
[void]$targets.Add([uint32]$main.Id)
foreach ($c in $children) { [void]$targets.Add([uint32]$c.ProcessId) }
$privSumB = (Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -ErrorAction SilentlyContinue |
    Where-Object { $targets.Contains([uint32]$_.IDProcess) } |
    Measure-Object -Property WorkingSetPrivate -Sum).Sum

Write-Output ("MainPID           : {0}" -f $main.Id)
Write-Output ("MainWorkingSetMB  : {0:N1}" -f ($mainWs / 1MB))
Write-Output ("MainPrivateMB     : {0:N1}" -f ($main.PrivateMemorySize64 / 1MB))
Write-Output ("WebView2ProcCount : {0}" -f $childCount)
Write-Output ("WebView2SetMB     : {0:N1}" -f ($childWs / 1MB))
Write-Output ("TotalWorkingSetMB : {0:N1}" -f (($mainWs + $childWs) / 1MB))
Write-Output ("TaskMgrStyleMB    : {0:N1}" -f ($privSumB / 1MB))
Write-Output ("WindowVisible     : {0}" -f ($main.MainWindowTitle.Length -gt 0))
