# Makes the_big_cheese.ico (the launcher's icon) from the cheese head in
# the_big_cheese.png: a square crop, at 256, 64, 48, 32 and 16 pixels.
#   powershell -ExecutionPolicy Bypass -File make-icon.ps1
Add-Type -AssemblyName System.Drawing
$here = $PSScriptRoot
$src = [System.Drawing.Image]::FromFile((Join-Path $here 'the_big_cheese.png'))
$crop = New-Object System.Drawing.Rectangle(62, 22, 156, 156)
$entries = foreach ($size in 256, 64, 48, 32, 16) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = 'HighQualityBicubic'
    $g.PixelOffsetMode = 'HighQuality'
    $g.DrawImage($src, (New-Object System.Drawing.Rectangle(0, 0, $size, $size)), $crop, 'Pixel')
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    if ($size -ge 256) {
        $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    } else {
        # Below 256 the classic form (32-bit BGRA bottom-up, then the AND
        # mask): some readers (System.Drawing, older shells) take only that.
        $bw = New-Object System.IO.BinaryWriter($ms)
        $bw.Write([uint32]40); $bw.Write([int32]$size); $bw.Write([int32]($size * 2))
        $bw.Write([uint16]1); $bw.Write([uint16]32); $bw.Write([uint32]0)
        $bw.Write([uint32]($size * $size * 4)); $bw.Write([int32]0); $bw.Write([int32]0)
        $bw.Write([uint32]0); $bw.Write([uint32]0)
        for ($y = $size - 1; $y -ge 0; $y--) {
            for ($x = 0; $x -lt $size; $x++) {
                $c = $bmp.GetPixel($x, $y)
                $bw.Write([byte]$c.B); $bw.Write([byte]$c.G); $bw.Write([byte]$c.R); $bw.Write([byte]$c.A)
            }
        }
        $maskRow = [int]([math]::Ceiling($size / 32) * 4)
        $bw.Write((New-Object byte[] ($maskRow * $size)))
        $bw.Flush()
    }
    $bmp.Dispose()
    [pscustomobject]@{ Size = $size; Bytes = $ms.ToArray() }
}
$src.Dispose()
# ICO: a header, one directory entry per size, then the PNG images.
$out = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter($out)
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$entries.Count)
$offset = 6 + 16 * $entries.Count
foreach ($e in $entries) {
    $dim = $(if ($e.Size -ge 256) { 0 } else { $e.Size })
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$e.Bytes.Length); $w.Write([uint32]$offset)
    $offset += $e.Bytes.Length
}
foreach ($e in $entries) { $w.Write($e.Bytes) }
$w.Flush()
[IO.File]::WriteAllBytes((Join-Path $here 'the_big_cheese.ico'), $out.ToArray())
'the_big_cheese.ico written'
