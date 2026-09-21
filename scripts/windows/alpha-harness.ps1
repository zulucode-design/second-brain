param(
  [ValidateSet('Metadata', 'EnsureTask', 'RunTask', 'RemoveTask', 'StopPid', 'FindProcess')][string]$Action,
  [string]$TaskName,
  [string]$ExecutablePath,
  [string]$TaskArguments,
  [int]$PidToStop
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

function Get-NowUtc { (Get-Date).ToUniversalTime().ToString('o') }

if (-not $Action -or -not $ExecutablePath) {
  throw 'Action and ExecutablePath are required'
}

$resolvedExecutable = [IO.Path]::GetFullPath($ExecutablePath)
$testRoot = [IO.Path]::GetFullPath('D:\SecondBrainTest') + '\'
if (-not $resolvedExecutable.StartsWith($testRoot, [StringComparison]::OrdinalIgnoreCase)) {
  throw "refusing executable outside D:\SecondBrainTest: $resolvedExecutable"
}
if (-not (Test-Path -LiteralPath $resolvedExecutable -PathType Leaf)) {
  throw "installed executable not found: $resolvedExecutable"
}

if ($Action -eq 'Metadata') {
  $osRecord = Get-CimInstance Win32_OperatingSystem
  $fileRecord = Get-Item -LiteralPath $resolvedExecutable
  [pscustomobject]@{
    at = Get-NowUtc
    action = 'metadata'
    osCaption = $osRecord.Caption
    osVersion = $osRecord.Version
    osArchitecture = $osRecord.OSArchitecture
    productVersion = $fileRecord.VersionInfo.ProductVersion
  } | ConvertTo-Json -Compress
  exit 0
}

if ($Action -eq 'EnsureTask') {
  if (-not $TaskName) { throw 'TaskName is required' }
  $interactiveUser = (Get-CimInstance Win32_ComputerSystem).UserName
  if (-not $interactiveUser) { throw 'no interactive Windows user is logged in' }
  $taskActionOptions = @{ Execute = $resolvedExecutable; WorkingDirectory = [IO.Path]::GetDirectoryName($resolvedExecutable) }
  if ($TaskArguments) { $taskActionOptions.Argument = $TaskArguments }
  $taskAction = New-ScheduledTaskAction @taskActionOptions
  $taskPrincipal = New-ScheduledTaskPrincipal -UserId $interactiveUser -LogonType Interactive -RunLevel Limited
  $taskSettings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 1)
  Register-ScheduledTask -TaskName $TaskName -Action $taskAction -Principal $taskPrincipal -Settings $taskSettings -Force | Out-Null
  [pscustomobject]@{ at = Get-NowUtc; action = 'ensure-task'; task = $TaskName; executable = $resolvedExecutable } |
    ConvertTo-Json -Compress
  exit 0
}

if ($Action -eq 'RunTask') {
  if (-not $TaskName) { throw 'TaskName is required' }
  $scheduledTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction Stop
  $taskExecutable = [IO.Path]::GetFullPath($scheduledTask.Actions[0].Execute.Trim('"'))
  if (-not $taskExecutable.Equals($resolvedExecutable, [StringComparison]::OrdinalIgnoreCase)) {
    throw "scheduled task executable differs: $taskExecutable"
  }
  Start-ScheduledTask -TaskName $TaskName
  [pscustomobject]@{ at = Get-NowUtc; action = 'run-task'; task = $TaskName; executable = $resolvedExecutable } |
    ConvertTo-Json -Compress
  exit 0
}

if ($Action -eq 'RemoveTask') {
  if (-not $TaskName) { throw 'TaskName is required' }
  $scheduledTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($scheduledTask) {
    $taskExecutable = [IO.Path]::GetFullPath($scheduledTask.Actions[0].Execute.Trim('"'))
    if (-not $taskExecutable.Equals($resolvedExecutable, [StringComparison]::OrdinalIgnoreCase)) {
      throw "refusing to remove task with executable $taskExecutable"
    }
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
  }
  [pscustomobject]@{ at = Get-NowUtc; action = 'remove-task'; task = $TaskName; removed = [bool]$scheduledTask } |
    ConvertTo-Json -Compress
  exit 0
}

if ($Action -eq 'FindProcess') {
  $escapedName = [IO.Path]::GetFileName($resolvedExecutable).Replace("'", "''")
  $matchingRecords = @(Get-CimInstance Win32_Process -Filter "Name = '$escapedName'" |
    Where-Object { $_.ExecutablePath -and [IO.Path]::GetFullPath($_.ExecutablePath).Equals($resolvedExecutable, [StringComparison]::OrdinalIgnoreCase) } |
    ForEach-Object {
      [pscustomobject]@{
        Id = $_.ProcessId
        ParentId = $_.ParentProcessId
        SessionId = $_.SessionId
        Started = $_.CreationDate.ToUniversalTime().ToString('o')
      }
    })
  [pscustomobject]@{ at = Get-NowUtc; action = 'find-process'; executable = $resolvedExecutable; processes = $matchingRecords } |
    ConvertTo-Json -Depth 3 -Compress
  exit 0
}

if ($PidToStop -le 0) { throw 'PidToStop is required' }
$processRecord = Get-CimInstance Win32_Process -Filter "ProcessId = $PidToStop"
if (-not $processRecord) { throw "process $PidToStop is not running" }
if (-not $processRecord.ExecutablePath) { throw "cannot verify executable path for process $PidToStop" }
$processExecutable = [IO.Path]::GetFullPath($processRecord.ExecutablePath)
if (-not $processExecutable.Equals($resolvedExecutable, [StringComparison]::OrdinalIgnoreCase)) {
  throw "process $PidToStop is $processExecutable, not $resolvedExecutable"
}
Stop-Process -Id $PidToStop -Force
Wait-Process -Id $PidToStop -Timeout 15 -ErrorAction SilentlyContinue
[pscustomobject]@{ at = Get-NowUtc; action = 'stop-pid'; pid = $PidToStop; executable = $processExecutable } |
  ConvertTo-Json -Compress
