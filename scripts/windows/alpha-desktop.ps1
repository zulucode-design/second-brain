# Runs inside the interactive desktop session (started by alpha-harness.ps1 DesktopScript), where
# injected input reaches the foreground and the screen can be captured, toasts included.
param(
  [ValidateSet('Screenshot', 'CaptureHotkey', 'NotionToken')][string]$Mode,
  # The screenshot to write, or for NotionToken the JSON result.
  [string]$OutputPath,
  [string]$Aumid,
  [string]$CredentialTarget,
  [switch]$RemoveCredential,
  # The banner was on screen from one to five seconds after the key press.
  [int]$SettleMilliseconds = 1500
)

$ErrorActionPreference = 'Stop'

$outputFull = [IO.Path]::GetFullPath($OutputPath)
if (-not $outputFull.StartsWith([IO.Path]::GetFullPath('D:\SecondBrainTest') + '\', [StringComparison]::OrdinalIgnoreCase)) {
  throw "refusing to write outside D:\SecondBrainTest: $outputFull"
}

# An SSH logon has no Credential Manager (cmdkey there says "A specified logon session does not
# exist"), so the run's Notion token is looked up here, and with -RemoveCredential deleted.
if ($Mode -eq 'NotionToken') {
  if (-not $CredentialTarget.StartsWith('integration:notion:')) { throw "not a Notion credential: $CredentialTarget" }
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace AlphaHarness {
  public static class Credentials {
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredReadW(string target, int type, int flags, out IntPtr credential);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredDeleteW(string target, int type, int flags);
    [DllImport("advapi32.dll")] static extern void CredFree(IntPtr buffer);
    // keyring stores its entries as generic credentials.
    const int Generic = 1;
    public static bool Exists(string target) {
      IntPtr found;
      if (!CredReadW(target, Generic, 0, out found)) return false;
      CredFree(found);
      return true;
    }
    public static void Delete(string target) { CredDeleteW(target, Generic, 0); }
  }
}
'@
  $credentialFound = [AlphaHarness.Credentials]::Exists($CredentialTarget)
  if ($credentialFound -and $RemoveCredential) { [AlphaHarness.Credentials]::Delete($CredentialTarget) }
  $credentialLeft = [AlphaHarness.Credentials]::Exists($CredentialTarget)
  [pscustomobject]@{ found = $credentialFound; left = $credentialLeft } | ConvertTo-Json -Compress |
    Set-Content -LiteralPath $outputFull -Encoding UTF8
  exit 0
}

Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace AlphaHarness {
  public static class Keys {
    [DllImport("user32.dll")] static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
    const uint KeyUp = 2;
    // Ctrl+Alt+N, the fixed capture trigger (ADR-0005).
    public static void CaptureHotkey() {
      byte[] chord = { 0x11, 0x12, 0x4E };
      foreach (var key in chord) keybd_event(key, 0, 0, UIntPtr.Zero);
      System.Threading.Thread.Sleep(80);
      Array.Reverse(chord);
      foreach (var key in chord) keybd_event(key, 0, KeyUp, UIntPtr.Zero);
    }
  }
}
'@

# The toasts Windows accepted from the app, read back from its notification history. Taken
# before and after the key press, so an older toast cannot pass for this one.
function Get-ToastTexts {
  if (-not $Aumid) { return @() }
  [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
  @([Windows.UI.Notifications.ToastNotificationManager]::History.GetHistory($Aumid) | ForEach-Object {
    ($_.Content.GetElementsByTagName('text') | ForEach-Object { $_.InnerText }) -join "`n"
  })
}

$toastsBefore = @(Get-ToastTexts)
if ($Mode -eq 'CaptureHotkey') { [AlphaHarness.Keys]::CaptureHotkey() }
Start-Sleep -Milliseconds $SettleMilliseconds

# The screen first: a toast banner shows for about five seconds, so reading the history before
# the capture let the banner leave the screenshot (run 20260926T050316Z).
$screenBounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bitmap = New-Object System.Drawing.Bitmap $screenBounds.Width, $screenBounds.Height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
try {
  $graphics.CopyFromScreen($screenBounds.Left, $screenBounds.Top, 0, 0, $bitmap.Size)
  $bitmap.Save($outputFull, [System.Drawing.Imaging.ImageFormat]::Png)
} finally {
  $graphics.Dispose()
  $bitmap.Dispose()
}

$toastsAfter = @(Get-ToastTexts)
if ($Aumid) {
  [pscustomobject]@{ aumid = $Aumid; before = $toastsBefore; after = $toastsAfter } | ConvertTo-Json -Depth 3 -Compress |
    Set-Content -LiteralPath ([IO.Path]::ChangeExtension($outputFull, '.json')) -Encoding UTF8
}
