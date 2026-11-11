# Generates app/icons/icon.ico + icon-256.png from the Aziz Playbook logo geometry
# (same shapes as ui/logo-playbook.svg). Usage: powershell -File scripts/make-icon.ps1
Add-Type -AssemblyName System.Drawing
$root = Split-Path $PSScriptRoot -Parent
$out = Join-Path $root "app\icons"
$S = 1024; $k = $S / 128.0
function P($x, $y) { New-Object Drawing.PointF([single]($x * $k), [single]($y * $k)) }
function Grad($x0, $y0, $x1, $y1, $c0, $c1, $diag) {
  $r = New-Object Drawing.RectangleF([single]($x0 * $k), [single]($y0 * $k), [single](($x1 - $x0) * $k), [single](($y1 - $y0) * $k))
  $mode = if ($diag) { [Drawing.Drawing2D.LinearGradientMode]::ForwardDiagonal } else { [Drawing.Drawing2D.LinearGradientMode]::Vertical }
  New-Object Drawing.Drawing2D.LinearGradientBrush($r, [Drawing.ColorTranslator]::FromHtml($c0), [Drawing.ColorTranslator]::FromHtml($c1), $mode)
}
$bmp = New-Object Drawing.Bitmap($S, $S, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = 'AntiAlias'; $g.Clear([Drawing.Color]::Transparent)

# badge background (rounded square, r=30)
$rr = 30 * $k * 2; $path = New-Object Drawing.Drawing2D.GraphicsPath
$path.AddArc(0, 0, $rr, $rr, 180, 90); $path.AddArc($S - $rr, 0, $rr, $rr, 270, 90)
$path.AddArc($S - $rr, $S - $rr, $rr, $rr, 0, 90); $path.AddArc(0, $S - $rr, $rr, $rr, 90, 90); $path.CloseFigure()
$bg = New-Object Drawing.Drawing2D.LinearGradientBrush((New-Object Drawing.Rectangle(0, 0, $S, $S)), [Drawing.ColorTranslator]::FromHtml('#2d1b69'), [Drawing.ColorTranslator]::FromHtml('#112a63'), [Drawing.Drawing2D.LinearGradientMode]::ForwardDiagonal)
$blend = New-Object Drawing.Drawing2D.ColorBlend(3)
$blend.Colors = @([Drawing.ColorTranslator]::FromHtml('#2d1b69'), [Drawing.ColorTranslator]::FromHtml('#0c0c26'), [Drawing.ColorTranslator]::FromHtml('#112a63'))
$blend.Positions = @([single]0, [single]0.5, [single]1); $bg.InterpolationColors = $blend
$g.FillPath($bg, $path)

function StrokePoly($pts, $brush) {
  $pen = New-Object Drawing.Pen($brush, [single](13 * $k)); $pen.LineJoin = 'Miter'; $pen.MiterLimit = 10
  $pen.StartCap = 'Flat'; $pen.EndCap = 'Flat'
  $g.DrawLines($pen, $pts)
}
$a = Grad 10 34 78 106 '#4b98ff' '#1f4fc4' $false
StrokePoly @((P 17 100), (P 44 40), (P 71 100)) $a
StrokePoly @((P 29 84), (P 59 84)) $a
$z = Grad 70 38 115 105 '#8a5cf0' '#5a2fb8' $false
StrokePoly @((P 76 45), (P 108 45), (P 76 98), (P 109 98)) $z
$gold = Grad 30 0 110 120 '#ffe08a' '#f29a1a' $false
$g.FillPolygon($gold, @((P 102 18), (P 66 68), (P 80 68), (P 38 118), (P 62 76), (P 49 76)))
$g.FillPolygon($gold, @((P 15 0), (P 39 0), (P 39 33), (P 27 25), (P 15 33)))
$g.FillPolygon((New-Object Drawing.SolidBrush([Drawing.ColorTranslator]::FromHtml('#14103a'))), @((P 23 8), (P 33 14), (P 23 20)))

function PngBytes($size) {
  $b = New-Object Drawing.Bitmap($size, $size, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $gg = [Drawing.Graphics]::FromImage($b); $gg.InterpolationMode = 'HighQualityBicubic'; $gg.SmoothingMode = 'HighQuality'; $gg.PixelOffsetMode = 'HighQuality'
  $gg.DrawImage($bmp, 0, 0, $size, $size); $gg.Dispose()
  $ms = New-Object IO.MemoryStream; $b.Save($ms, [Drawing.Imaging.ImageFormat]::Png); $b.Dispose(); ,$ms.ToArray()
}
$sizes = 256, 128, 64, 48, 32, 24, 16
$pngs = $sizes | ForEach-Object { , (PngBytes $_) }
[IO.File]::WriteAllBytes((Join-Path $out "icon-256.png"), $pngs[0])
$ms = New-Object IO.MemoryStream; $w = New-Object IO.BinaryWriter($ms)
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
  $s = $sizes[$i]; $dim = if ($s -ge 256) { 0 } else { $s }
  $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
  $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$pngs[$i].Length); $w.Write([uint32]$offset)
  $offset += $pngs[$i].Length
}
foreach ($p in $pngs) { $w.Write($p) }
[IO.File]::WriteAllBytes((Join-Path $out "icon.ico"), $ms.ToArray())
$g.Dispose(); $bmp.Dispose()
"icon.ico written: $((Get-Item (Join-Path $out 'icon.ico')).Length) bytes"
