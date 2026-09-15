# Force-stop one Second Brain process for the issue #97 interruption protocol, or report
# which of its processes are still running.
#
#   .\scripts\windows\sync-interrupt.ps1 -Action Stop -Target App
#   .\scripts\windows\sync-interrupt.ps1 -Action Stop -Target Sidecar
#   .\scripts\windows\sync-interrupt.ps1 -Action Report
#
# Processes are matched by exact executable path inside the installed package, never by name
# pattern: a development build, another checkout, or an unrelated Syncthing on the same machine
# must not be touched. Both installer layouts are searched: NSIS installs per user under
# %LOCALAPPDATA%, MSI per machine under Program Files. Pass -InstallDir for anything else.
#
# Stop refuses unless exactly one process matches. The sync watchdog runs from the same
# helixnotes.exe with --helix-sync-watchdog, so App excludes it; stopping the app is supposed to
# leave the watchdog to shut the sidecar down, which Report then shows.
#
# A process whose executable path Windows will not reveal to this session (typically an
# elevated one) cannot be proven to be the packaged build, so it is reported as Unverified and
# makes Stop refuse rather than read as "not running".
#
# Exit codes: 0 done, 1 refused (match count or unverified process), 2 usage error.

param(
  [ValidateSet('Stop', 'Report')][string]$Action,
  [ValidateSet('App', 'Sidecar')][string]$Target,
  [string[]]$InstallDir = @(
    (Join-Path $env:LOCALAPPDATA 'HelixNotes'),
    (Join-Path $env:ProgramFiles 'HelixNotes')
  )
)

$ErrorActionPreference = 'Stop'

if (-not $Action -or ($Action -eq 'Stop' -and -not $Target)) {
  [Console]::Error.WriteLine('usage: sync-interrupt.ps1 -Action Report | -Action Stop -Target App|Sidecar [-InstallDir <dir>]')
  exit 2
}

$appExes = @($InstallDir | ForEach-Object { [IO.Path]::GetFullPath((Join-Path $_ 'helixnotes.exe')) })
$sidecarExes = @($InstallDir | ForEach-Object { [IO.Path]::GetFullPath((Join-Path $_ 'syncthing.exe')) })

# Syncthing runs as a monitor process (started by the app, and the one it supervises) plus a
# worker it spawns itself. Only the monitor, whose parent is the packaged app, is the Sidecar
# target. A packaged Syncthing whose parent is another packaged Syncthing is its SidecarWorker;
# one whose parent is neither is a SidecarOrphan, which is what outliving a stop looks like.
function Get-PackagedProcesses {
  $all = @(Get-CimInstance Win32_Process -Filter "Name = 'helixnotes.exe' OR Name = 'syncthing.exe'")
  $sidecarIds = @($all | Where-Object { $sidecarExes -contains $_.ExecutablePath } | ForEach-Object ProcessId)
  $appIds = @($all | Where-Object { $appExes -contains $_.ExecutablePath } | ForEach-Object ProcessId)
  $all |
    ForEach-Object {
      $path = $_.ExecutablePath
      $role = if (-not $path) { 'Unverified' }
              elseif ($sidecarExes -contains $path -and $sidecarIds -contains $_.ParentProcessId) { 'SidecarWorker' }
              elseif ($sidecarExes -contains $path -and $appIds -contains $_.ParentProcessId) { 'Sidecar' }
              elseif ($sidecarExes -contains $path) { 'SidecarOrphan' }
              elseif ($appExes -notcontains $path) { $null }
              elseif ($_.CommandLine -like '*--helix-sync-watchdog*') { 'Watchdog' }
              else { 'App' }
      if ($role) {
        [pscustomobject]@{ Role = $role; Name = $_.Name; Id = $_.ProcessId; Path = $path; Started = $_.CreationDate.ToUniversalTime().ToString('o') }
      }
    }
}

function Get-Now { (Get-Date).ToUniversalTime().ToString('o') }

if ($Action -eq 'Report') {
  [pscustomobject]@{ at = Get-Now; action = 'report'; installDir = $InstallDir; processes = @(Get-PackagedProcesses) } |
    ConvertTo-Json -Depth 4 -Compress
  exit 0
}

$processes = @(Get-PackagedProcesses)
$unverified = @($processes | Where-Object Role -eq 'Unverified')
$found = @($processes | Where-Object Role -eq $Target)
if ($unverified.Count -gt 0 -or $found.Count -ne 1) {
  [Console]::Error.WriteLine("expected exactly one packaged $Target process and no unverified ones; found $($found.Count) and $($unverified.Count) unverified (try an elevated shell); nothing stopped")
  exit 1
}

Stop-Process -Id $found[0].Id -Force
$stoppedAt = Get-Now
Start-Sleep -Milliseconds 500
[pscustomobject]@{ at = $stoppedAt; action = 'stop'; target = $Target; pid = $found[0].Id; path = $found[0].Path; processesAfter = @(Get-PackagedProcesses) } |
  ConvertTo-Json -Depth 4 -Compress
