param(
  [ValidateSet('Isolate', 'State', 'Restore')][string]$Action,
  [string]$RunId,
  # Program paths separated by '|': powershell -File passes one string per parameter.
  [string]$ProgramPath,
  [int]$DeadlineMinutes = 20
)

# #28 acceptance gate: blocks the installed app's programs from every remote address outside the
# Tailnet and loopback, so a sync run proves it needs no internet. Only programs this run names
# are touched; the rest of the desktop keeps its connection. Every rule carries this run's group,
# and a SYSTEM task armed before the first rule removes the group at the deadline even if the
# controller never returns.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Set-StrictMode -Version Latest

if ($RunId -notmatch '^\d{8}T\d{6}Z$') { throw "invalid run id: $RunId" }
$ruleGroup = "SecondBrainAcceptance-$RunId"
$rollbackTask = "SecondBrainAcceptanceRollback-$RunId"

# Everything except 100.64.0.0/10 (Tailscale IPv4) and 127.0.0.0/8; everything except ::1 and
# fd7a:115c:a1e0::/48 (Tailscale IPv6). Block rules win over allow rules, so the exceptions are
# left out of the ranges instead of allowed separately.
$blockedRanges = @(
  '0.0.0.0-100.63.255.255',
  '100.128.0.0-126.255.255.255',
  '128.0.0.0-255.255.255.255',
  '::',
  '::2-fd7a:115c:a1df:ffff:ffff:ffff:ffff:ffff',
  'fd7a:115c:a1e1::-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff'
)

function Get-IsolationState {
  $rules = @(Get-NetFirewallRule -Group $ruleGroup -ErrorAction SilentlyContinue | ForEach-Object {
    [pscustomobject]@{
      name = $_.Name
      enabled = [string]$_.Enabled
      action = [string]$_.Action
      direction = [string]$_.Direction
      program = ($_ | Get-NetFirewallApplicationFilter).Program
      remote = @(($_ | Get-NetFirewallAddressFilter).RemoteAddress)
    }
  })
  [pscustomobject]@{
    at = (Get-Date).ToUniversalTime().ToString('o')
    group = $ruleGroup
    rules = $rules
    rollbackArmed = [bool](Get-ScheduledTask -TaskName $rollbackTask -ErrorAction SilentlyContinue)
  }
}

if ($Action -eq 'Isolate') {
  $programFiles = @($ProgramPath -split '\|' | Where-Object { $_ })
  if (-not $programFiles.Count) { throw 'ProgramPath is required' }
  if (@(Get-NetFirewallRule -Group $ruleGroup -ErrorAction SilentlyContinue).Count) { throw "rules already exist for $ruleGroup" }
  foreach ($programFile in $programFiles) {
    if (-not (Test-Path -LiteralPath $programFile -PathType Leaf)) { throw "program not found: $programFile" }
  }
  # Armed first, so no rule ever exists without its removal scheduled.
  $removeCommand = "Get-NetFirewallRule -Group '$ruleGroup' -ErrorAction SilentlyContinue | Remove-NetFirewallRule; Unregister-ScheduledTask -TaskName '$rollbackTask' -Confirm:`$false"
  $taskAction = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -NonInteractive -Command `"$removeCommand`""
  $taskTrigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes($DeadlineMinutes)
  $taskPrincipal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
  Register-ScheduledTask -TaskName $rollbackTask -Action $taskAction -Trigger $taskTrigger -Principal $taskPrincipal -Force | Out-Null
  $index = 0
  foreach ($programFile in $programFiles) {
    $index += 1
    New-NetFirewallRule -Name "$ruleGroup-$index" -DisplayName "Second Brain acceptance $RunId $index" -Group $ruleGroup `
      -Direction Outbound -Action Block -Program $programFile -RemoteAddress $blockedRanges -Profile Any | Out-Null
  }
  Get-IsolationState | ConvertTo-Json -Compress -Depth 5
  exit 0
}

if ($Action -eq 'State') {
  Get-IsolationState | ConvertTo-Json -Compress -Depth 5
  exit 0
}

if ($Action -eq 'Restore') {
  Get-NetFirewallRule -Group $ruleGroup -ErrorAction SilentlyContinue | Remove-NetFirewallRule
  if (Get-ScheduledTask -TaskName $rollbackTask -ErrorAction SilentlyContinue) {
    Unregister-ScheduledTask -TaskName $rollbackTask -Confirm:$false
  }
  $state = Get-IsolationState
  if ($state.rules.Count -or $state.rollbackArmed) { throw "isolation left behind: $($state | ConvertTo-Json -Compress -Depth 5)" }
  $state | ConvertTo-Json -Compress -Depth 5
  exit 0
}

throw "unknown action: $Action"
