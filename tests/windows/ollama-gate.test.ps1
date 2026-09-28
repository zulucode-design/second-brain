$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$sourcePath = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../scripts/windows/owned-process.cs'))
Add-Type -Path $sourcePath
$shellPath = (Get-Command powershell.exe).Source
$testDirectory = Join-Path $env:TEMP ('sb-ollama-job-' + [guid]::NewGuid())
New-Item -ItemType Directory $testDirectory | Out-Null

function Assert-UnderTemp($p) {
  $full = [IO.Path]::GetFullPath($p)
  if (-not $full.StartsWith([IO.Path]::GetFullPath($env:TEMP) + '\')) { throw "refusing to delete $full" }
}
function Encode($text) { [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($text)) }
function Wait-ForPidFile($path) {
  $deadline = (Get-Date).AddSeconds(15)
  while ((Get-Date) -lt $deadline) {
    if (Test-Path $path) {
      $value = Get-Content $path -Raw
      if ($value -match '^\d+\s*$') { return [int]$value }
    }
    Start-Sleep -Milliseconds 100
  }
  throw "No child PID in $path"
}
function Assert-Stopped($ownedId) {
  $remaining = Get-Process -Id $ownedId -ErrorAction SilentlyContinue
  if ($remaining -and -not $remaining.WaitForExit(5000)) { throw "Leaked process $ownedId" }
}

$unrelated = Start-Process $shellPath -ArgumentList '-NoProfile -Command "Start-Sleep 60"' -PassThru
try {
  foreach ($scenario in 'normal-cleanup', 'parent-exits', 'supervisor-killed') {
    $childFile = Join-Path $testDirectory "$scenario.pid"
    $childCode = Encode 'Start-Sleep 60'
    $parentCode = "`$ownedChild = Start-Process '$shellPath' -ArgumentList '-NoProfile -EncodedCommand $childCode' -PassThru; [IO.File]::WriteAllText('$childFile', [string]`$ownedChild.Id)"
    if ($scenario -ne 'parent-exits') { $parentCode += '; Start-Sleep 60' }
    $arguments = '-NoProfile -EncodedCommand ' + (Encode $parentCode)
    $owned = $null
    $supervisor = $null
    try {
      if ($scenario -eq 'supervisor-killed') {
        $parentFile = Join-Path $testDirectory 'parent.pid'
        $supervisorCode = "Add-Type -Path '$sourcePath'; `$owned = [OwnedProcess]::new('$shellPath', '$arguments'); [IO.File]::WriteAllText('$parentFile', [string]`$owned.Id); Start-Sleep 60"
        $supervisor = Start-Process $shellPath -ArgumentList ('-NoProfile -EncodedCommand ' + (Encode $supervisorCode)) -PassThru
        $parentId = Wait-ForPidFile $parentFile
      } else {
        $owned = [OwnedProcess]::new($shellPath, $arguments)
        $parentId = $owned.Id
      }
      $childId = Wait-ForPidFile $childFile
      if ($scenario -eq 'parent-exits') {
        if (-not $owned.Wait(5000)) { throw 'Parent did not exit' }
        if (-not (Get-Process -Id $childId -ErrorAction SilentlyContinue)) { throw 'Repro did not leave a live child' }
      }
      if ($supervisor) { Stop-Process -Id $supervisor.Id -Force; $supervisor.WaitForExit() }
      if ($owned) { $owned.Dispose() }
      Assert-Stopped $parentId
      Assert-Stopped $childId
      if ($unrelated.HasExited) { throw 'Unrelated process was terminated' }
      Write-Output "PASS ${scenario}: parent and descendant gone; unrelated process preserved"
    } finally {
      if ($owned) { $owned.Dispose() }
      if ($supervisor -and -not $supervisor.HasExited) { Stop-Process -Id $supervisor.Id -Force }
    }
  }
} finally {
  if (-not $unrelated.HasExited) { Stop-Process -Id $unrelated.Id -Force }
  Assert-UnderTemp $testDirectory
  Remove-Item -LiteralPath $testDirectory -Recurse -Force
}
