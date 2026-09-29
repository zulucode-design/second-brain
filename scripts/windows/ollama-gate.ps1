param(
  [ValidateRange(1024, 65535)][int]$Port = 11436,
  [ValidateRange(1, 7200)][int]$LifetimeSeconds = 3600
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
Add-Type -Path (Join-Path $PSScriptRoot 'owned-process.cs')
$ollamaExecutable = (Get-Command ollama.exe -ErrorAction Stop).Source

# A busy endpoint belongs to someone else. Never adopt or stop its owner.
$portProbe = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $Port)
try { $portProbe.Start() } finally { $portProbe.Stop() }

$previousHost = $env:OLLAMA_HOST
$ownedServer = $null
try {
  $env:OLLAMA_HOST = "127.0.0.1:$Port"
  $ownedServer = [OwnedProcess]::new($ollamaExecutable, 'serve')
  [pscustomobject]@{
    supervisorPid = [Diagnostics.Process]::GetCurrentProcess().Id
    serverPid = $ownedServer.Id
    endpoint = "http://127.0.0.1:$Port"
    lifetimeSeconds = $LifetimeSeconds
  } | ConvertTo-Json -Compress
  # This is a lease, not a daemon: an abandoned SSH session expires too.
  if ($ownedServer.Wait($LifetimeSeconds * 1000)) {
    throw 'The gate Ollama server exited before its lease ended.'
  }
} finally {
  if ($ownedServer) { $ownedServer.Dispose() }
  $env:OLLAMA_HOST = $previousHost
}
