# Runs inside the interactive desktop session (started by alpha-harness.ps1 DesktopScript), where
# injected input reaches the foreground and the screen can be captured, toasts included.
param(
  [ValidateSet('Screenshot', 'CaptureHotkey')][string]$Mode,
  [string]$ImagePath,
  [string]$Aumid,
  [int]$SettleMilliseconds = 3000
)

$ErrorActionPreference = 'Stop'

$imageFull = [IO.Path]::GetFullPath($ImagePath)
if (-not $imageFull.StartsWith([IO.Path]::GetFullPath('D:\SecondBrainTest') + '\', [StringComparison]::OrdinalIgnoreCase)) {
  throw "refusing to write outside D:\SecondBrainTest: $imageFull"
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
$toastsAfter = @(Get-ToastTexts)
if ($Aumid) {
  [pscustomobject]@{ aumid = $Aumid; before = $toastsBefore; after = $toastsAfter } | ConvertTo-Json -Depth 3 -Compress |
    Set-Content -LiteralPath ([IO.Path]::ChangeExtension($imageFull, '.json')) -Encoding UTF8
}

$screenBounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
$bitmap = New-Object System.Drawing.Bitmap $screenBounds.Width, $screenBounds.Height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
try {
  $graphics.CopyFromScreen($screenBounds.Left, $screenBounds.Top, 0, 0, $bitmap.Size)
  $bitmap.Save($imageFull, [System.Drawing.Imaging.ImageFormat]::Png)
} finally {
  $graphics.Dispose()
  $bitmap.Dispose()
}
