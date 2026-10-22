# Desktop Calendar - app window screenshot (acceptance helper)
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File screenshot-app.ps1
# Brings the app window to foreground, captures the primary screen to %TEMP%\dc-screen.png
# NOTE: keep this file pure ASCII (PowerShell 5.1 ANSI/GBK parsing pitfall).

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class DpiHelper {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
}
"@
[void][DpiHelper]::SetProcessDPIAware()

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$p = Get-Process desktop-calendar -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p -and $p.MainWindowHandle -ne [IntPtr]::Zero) {
    [void][DpiHelper]::SetForegroundWindow($p.MainWindowHandle)
    Start-Sleep -Milliseconds 500
} else {
    Write-Output "NOTE: app window not found; capturing screen anyway"
}

$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)

$out = Join-Path $env:TEMP 'dc-screen.png'
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()
Write-Output ("SAVED: " + $out)
