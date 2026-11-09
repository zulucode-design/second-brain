# Runs inside the interactive desktop session (started by alpha-harness.ps1 DesktopScript), where
# injected input reaches the foreground and the screen can be captured, toasts included.
param(
  [ValidateSet('Screenshot', 'CaptureHotkey', 'NotionToken', 'LaunchShortcut')][string]$Mode,
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

if ($Mode -eq 'LaunchShortcut') {
  $shortcutPath = Join-Path ([Environment]::GetFolderPath('Programs')) 'Second Brain.lnk'
  if (-not (Test-Path -LiteralPath $shortcutPath -PathType Leaf)) { throw "Start-menu entry missing: $shortcutPath" }
  Start-Process -FilePath $shortcutPath
  [pscustomobject]@{ shortcut = $shortcutPath } | ConvertTo-Json -Compress |
    Set-Content -LiteralPath $outputFull -Encoding UTF8
  exit 0
}

# An SSH logon has no Credential Manager (cmdkey there says "A specified logon session does not
# exist"), so the credential is looked up here, and with -RemoveCredential deleted. The mode name
# is the Notion token's from before the AI keys (#252) joined it. It answers only whether the
# credential exists: the secret is never returned, written or logged, and the copy Windows hands
# back is overwritten before it is freed. A Credential Manager call that fails for any reason but
# ERROR_NOT_FOUND stops the mode before it answers (Assert-CredentialCall below).
if ($Mode -eq 'NotionToken') {
  # The credentials this mode may touch: the run's Notion token, and the app's four AI key
  # entries. keyring names a Windows credential "<account>.<service>" (src-tauri/src/secret_store.rs
  # lists the accounts). Every other target is refused.
  $appService = 'io.github.zulucodedesign.SecondBrain'
  $aiAccounts = @('ai:anthropic', 'ai:openai', 'ai:ollama', 'ai:openai-compatible')
  $aiTargets = foreach ($aiAccount in $aiAccounts) { '{0}.{1}' -f $aiAccount, $appService }
  $isNotionToken = $CredentialTarget.StartsWith('integration:notion:')
  if (-not $isNotionToken -and $aiTargets -cnotcontains $CredentialTarget) {
    throw "not a credential the harness manages: $CredentialTarget"
  }
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace AlphaHarness {
  public static class Credentials {
    // CREDENTIALW, with the strings left as pointers so that nothing is copied out of it. The
    // secret's own buffer is reached only to overwrite it.
    [StructLayout(LayoutKind.Sequential)]
    struct CredentialRecord {
      public uint Flags;
      public uint Type;
      public IntPtr TargetName;
      public IntPtr Comment;
      public uint LastWrittenLow;
      public uint LastWrittenHigh;
      public uint CredentialBlobSize;
      public IntPtr CredentialBlob;
      public uint Persist;
      public uint AttributeCount;
      public IntPtr Attributes;
      public IntPtr TargetAlias;
      public IntPtr UserName;
    }
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredReadW(string target, int type, int flags, out IntPtr credential);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredDeleteW(string target, int type, int flags);
    [DllImport("advapi32.dll")] static extern void CredFree(IntPtr buffer);
    // keyring stores its entries as generic credentials.
    const int Generic = 1;
    // The Win32 error of the call that just failed. One that set none still failed.
    static int LastError() {
      int code = Marshal.GetLastWin32Error();
      return code == 0 ? -1 : code;
    }
    // 0 when the credential exists, otherwise the Win32 error CredReadW failed with:
    // ERROR_NOT_FOUND (1168) when there is no such credential, anything else when it could not be read.
    public static int Read(string target) {
      IntPtr found;
      if (!CredReadW(target, Generic, 0, out found)) return LastError();
      try {
        var record = (CredentialRecord)Marshal.PtrToStructure(found, typeof(CredentialRecord));
        if (record.CredentialBlob != IntPtr.Zero) {
          for (int offset = 0; offset < record.CredentialBlobSize; offset++) {
            Marshal.WriteByte(record.CredentialBlob, offset, 0);
          }
        }
      } finally {
        CredFree(found);
      }
      return 0;
    }
    // 0 when the credential was deleted, otherwise the Win32 error CredDeleteW failed with.
    public static int Delete(string target) {
      return CredDeleteW(target, Generic, 0) ? 0 : LastError();
    }
  }
}
'@
  # ERROR_NOT_FOUND is the one failure that means "not there". Any other stops the mode before it
  # writes found or left: it writes the call and its error code, nothing from the credential, and
  # exits 1, so the harness reads it as "could not check" and never as absent (#252).
  $notFoundCode = 1168
  function Assert-CredentialCall([string]$callName, [int]$statusCode) {
    if ($statusCode -eq 0 -or $statusCode -eq $notFoundCode) { return }
    [pscustomobject]@{ error = "$callName failed with Win32 error $statusCode" } | ConvertTo-Json -Compress |
      Set-Content -LiteralPath $outputFull -Encoding UTF8
    exit 1
  }
  $readStatus = [AlphaHarness.Credentials]::Read($CredentialTarget)
  Assert-CredentialCall 'CredReadW' $readStatus
  $credentialFound = $readStatus -eq 0
  if ($credentialFound -and $RemoveCredential) {
    Assert-CredentialCall 'CredDeleteW' ([AlphaHarness.Credentials]::Delete($CredentialTarget))
  }
  $leftStatus = [AlphaHarness.Credentials]::Read($CredentialTarget)
  Assert-CredentialCall 'CredReadW' $leftStatus
  $credentialLeft = $leftStatus -eq 0
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
# Pressed twice, with the history read between: the second toast has to replace the first rather
# than stack under it (#153). The gap lets the first toast reach the history.
$toastsAfterFirst = $null
if ($Mode -eq 'CaptureHotkey') {
  [AlphaHarness.Keys]::CaptureHotkey()
  Start-Sleep -Seconds 3
  $toastsAfterFirst = @(Get-ToastTexts)
  [AlphaHarness.Keys]::CaptureHotkey()
}
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
  [pscustomobject]@{ aumid = $Aumid; before = $toastsBefore; afterFirst = $toastsAfterFirst; after = $toastsAfter } | ConvertTo-Json -Depth 3 -Compress |
    Set-Content -LiteralPath ([IO.Path]::ChangeExtension($outputFull, '.json')) -Encoding UTF8
}
