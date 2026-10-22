# Compose src-tauri/icons/tray-dot.png = app icon (32x32) + red badge dot (bottom-right).
# ASCII ONLY - do not insert non-ASCII chars (PowerShell 5.1 reads no-BOM files as ANSI).
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Drawing

$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$iconsDir = Join-Path (Split-Path -Parent $toolsDir) "src-tauri\icons"
$srcPath = Join-Path $iconsDir "icon.png"
$dstPath = Join-Path $iconsDir "tray-dot.png"

$size = 32
$src = [System.Drawing.Image]::FromFile($srcPath)
$bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.DrawImage($src, 0, 0, $size, $size)
$g.Dispose()
$src.Dispose()

$ring = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::White)
$dot = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(229, 57, 53))
$g2 = [System.Drawing.Graphics]::FromImage($bmp)
$g2.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g2.FillEllipse($ring, 16, 16, 16, 16)
$g2.FillEllipse($dot, 18, 18, 12, 12)
$g2.Dispose()
$ring.Dispose()
$dot.Dispose()

$bmp.Save($dstPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

Write-Output ("saved: " + $dstPath)
