param(
  [ValidateSet('StartStartup', 'StartDriver', 'StopDriver', 'Startup', 'Driver')][string]$Mode,
  [string]$LogPath
)

$ErrorActionPreference = 'Stop'
$root = 'D:\SecondBrainTest\sb88\perf'
$fullLog = [IO.Path]::GetFullPath($LogPath)
if (-not $fullLog.StartsWith([IO.Path]::GetFullPath($root) + '\', [StringComparison]::OrdinalIgnoreCase)) {
  throw "performance log must be under $root"
}
New-Item -ItemType Directory -Force ([IO.Path]::GetDirectoryName($fullLog)) | Out-Null
$taskName = 'SecondBrainPerf88'
$driverExecutable = 'D:\SecondBrainTest\sb88\driver\bin\tauri-driver.exe'
$nativeExecutable = 'D:\SecondBrainTest\sb88\driver\msedge\msedgedriver.exe'
$appExecutable = 'D:\SecondBrainTest\app\second-brain.exe'

if ($Mode -eq 'StartStartup' -or $Mode -eq 'StartDriver') {
  $existingTask = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
  if ($existingTask -and $existingTask.State -eq 'Running') { throw "$taskName already runs" }
  $interactiveUser = (Get-CimInstance Win32_ComputerSystem).UserName
  if (-not $interactiveUser) { throw 'no interactive Windows user is logged in' }
  $childMode = if ($Mode -eq 'StartStartup') { 'Startup' } else { 'Driver' }
  $shellExecutable = (Get-Command powershell.exe).Source
  $taskArguments = "-NoProfile -File `"$PSCommandPath`" -Mode $childMode -LogPath `"$fullLog`""
  $taskAction = New-ScheduledTaskAction -Execute $shellExecutable -Argument $taskArguments -WorkingDirectory $PSScriptRoot
  $taskPrincipal = New-ScheduledTaskPrincipal -UserId $interactiveUser -LogonType Interactive -RunLevel Limited
  $taskSettings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 1)
  Register-ScheduledTask -TaskName $taskName -Action $taskAction -Principal $taskPrincipal -Settings $taskSettings -Force | Out-Null
  Start-ScheduledTask -TaskName $taskName
  Write-Output "started $taskName in interactive session: $childMode"
  exit 0
}

if ($Mode -eq 'StopDriver') {
  $driverRecord = Get-Content "$fullLog.driver.json" -Raw | ConvertFrom-Json
  if (-not $driverRecord.executable.Equals($driverExecutable, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'driver record names another executable'
  }
  $driverProcess = Get-Process -Id $driverRecord.pid -ErrorAction SilentlyContinue
  if ($driverProcess) {
    if (-not $driverProcess.Path.Equals($driverExecutable, [StringComparison]::OrdinalIgnoreCase) -or
        [Math]::Abs(($driverProcess.StartTime.ToUniversalTime() - [datetime]::Parse($driverRecord.started).ToUniversalTime()).TotalSeconds) -gt 1) {
      throw 'driver PID was reused'
    }
    $nativeChildren = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($driverRecord.pid)")
    foreach ($nativeChild in $nativeChildren) {
      if ($nativeChild.ExecutablePath.Equals('C:\Windows\System32\conhost.exe', [StringComparison]::OrdinalIgnoreCase)) {
        continue
      }
      if (-not $nativeChild.ExecutablePath.Equals($nativeExecutable, [StringComparison]::OrdinalIgnoreCase)) {
        throw "unknown driver child $($nativeChild.ProcessId)"
      }
      $appChildren = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($nativeChild.ProcessId)")
      foreach ($appChild in $appChildren) {
        if (-not $appChild.ExecutablePath.Equals($appExecutable, [StringComparison]::OrdinalIgnoreCase)) {
          throw "unknown native-driver child $($appChild.ProcessId)"
        }
        Stop-Process -Id $appChild.ProcessId -Force
      }
      Stop-Process -Id $nativeChild.ProcessId -Force
    }
    Stop-Process -Id $driverRecord.pid -Force
  }
  Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
  Write-Output 'driver stopped'
  exit 0
}

if ($Mode -eq 'Startup') {
  $startupScript = Join-Path $PSScriptRoot 'perf-startup.mjs'
  & (Get-Command node.exe).Source $startupScript $appExecutable $fullLog 5
  if ($LASTEXITCODE -ne 0) { throw "startup probe exited $LASTEXITCODE" }
  exit 0
}

$env:SECOND_BRAIN_PERF_LOG = $fullLog
$launchedDriver = Start-Process -FilePath $driverExecutable `
  -ArgumentList @('--port', '4444', '--native-driver', "`"$nativeExecutable`"") `
  -RedirectStandardOutput "$fullLog.driver.out" -RedirectStandardError "$fullLog.driver.err" -PassThru
@{ pid = $launchedDriver.Id; executable = $driverExecutable; started = $launchedDriver.StartTime.ToUniversalTime().ToString('o') } |
  ConvertTo-Json -Compress | Set-Content "$fullLog.driver.json"
$launchedDriver.WaitForExit()
if ($launchedDriver.ExitCode -ne 0) { throw "tauri-driver exited $($launchedDriver.ExitCode)" }
