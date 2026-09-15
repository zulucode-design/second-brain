# Force-stop one Second Brain process for the issue #97 interruption protocol, or report
# which of its processes are still running.
#
#   .\scripts\windows\sync-interrupt.ps1 -Action Stop -Target App
#   .\scripts\windows\sync-interrupt.ps1 -Action Stop -Target Sidecar
#   .\scripts\windows\sync-interrupt.ps1 -Action Report
#
# Processes are matched by exact executable path inside the installed package, never by name
# pattern: a development build, another checkout, or an unrelated Syncthing on the same machine
# must not be touched. Stop refuses unless exactly one process matches. The sync watchdog runs
# from the same helixnotes.exe with --helix-sync-watchdog, so App excludes it; stopping the app
# is supposed to leave the watchdog to shut the sidecar down, which Report then shows.

param(
  [Parameter(Mandatory = $true)][ValidateSet('Stop', 'Report')][string]$Action,
  [ValidateSet('App', 'Sidecar')][string]$Target,
  [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'HelixNotes')
)

$ErrorActionPreference = 'Stop'
$appExe = [IO.Path]::GetFullPath((Join-Path $InstallDir 'helixnotes.exe'))
$sidecarExe = [IO.Path]::GetFullPath((Join-Path $InstallDir 'syncthing.exe'))

function Get-PackagedProcesses {
  Get-CimInstance Win32_Process |
    Where-Object { $_.ExecutablePath -and ($_.ExecutablePath -ieq $appExe -or $_.ExecutablePath -ieq $sidecarExe) } |
    ForEach-Object {
      $role = if ($_.ExecutablePath -ieq $sidecarExe) { 'Sidecar' }
              elseif ($_.CommandLine -like '*--helix-sync-watchdog*') { 'Watchdog' }
              else { 'App' }
      [pscustomobject]@{ Role = $role; Id = $_.ProcessId; Started = $_.CreationDate.ToUniversalTime().ToString('o') }
    }
}

$now = (Get-Date).ToUniversalTime().ToString('o')

if ($Action -eq 'Report') {
  [pscustomobject]@{ at = $now; action = 'report'; installDir = $InstallDir; processes = @(Get-PackagedProcesses) } |
    ConvertTo-Json -Depth 4 -Compress
  exit 0
}

if (-not $Target) { throw '-Target App|Sidecar is required with -Action Stop' }
$found = @(Get-PackagedProcesses | Where-Object Role -eq $Target)
if ($found.Count -ne 1) {
  [Console]::Error.WriteLine("expected exactly one packaged $Target process, found $($found.Count); nothing stopped")
  exit 1
}

Stop-Process -Id $found[0].Id -Force
Start-Sleep -Milliseconds 500
[pscustomobject]@{ at = $now; action = 'stop'; target = $Target; pid = $found[0].Id; processesAfter = @(Get-PackagedProcesses) } |
  ConvertTo-Json -Depth 4 -Compress
