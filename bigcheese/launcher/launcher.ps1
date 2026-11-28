# VIctory-BigCheese launcher: page 1 the game folder, page 2 the options and
# Play. It only picks one of the .cmd launchers next to it and sets a few
# settings for that run; everything else is theirs, so the crash report and
# the log still show in the black window. Started by BigCheese-Launcher.cmd.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

$here = $PSScriptRoot
$root = Split-Path $here -Parent
$exeName = 'game_scarlett_master.exe'
$settingsFile = Join-Path $here 'settings.txt'
$folderFile = Join-Path $root 'game-folder.txt'

# The zip's names, or the play folder's.
function Find-Launcher([string[]]$names) {
    foreach ($dir in (Join-Path $root 'Other-Launchers'), $root) {
        foreach ($n in $names) { $p = Join-Path $dir $n; if (Test-Path $p) { return $p } }
    }
    return $null
}
$normalCmd = Find-Launcher @('Play.cmd', 'Play-BigCheese-AMD.cmd')
$smallCmd = Find-Launcher @('Play-16GB-RAM.cmd', 'Play-BigCheese-AMD-16GB-RAM.cmd')

# ------------------------------------------------------------ settings
$ramGb = 16
try { $ramGb = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB) } catch {}
$settings = @{ rt = '0'; ram16 = $(if ($ramGb -le 24) { '1' } else { '0' }); vram8 = '0'; console = 'series-x' }
if (Test-Path $settingsFile) {
    foreach ($line in Get-Content $settingsFile) {
        if ($line -match '^(\w+)=(.*)$') { $settings[$Matches[1]] = $Matches[2].Trim() }
    }
}
function Save-Settings {
    $settings.GetEnumerator() | Sort-Object Name | ForEach-Object { "$($_.Name)=$($_.Value)" } |
        Set-Content -Path $settingsFile -Encoding ASCII
}

$gameDir = ''
if (Test-Path $folderFile) { $gameDir = ((Get-Content $folderFile -TotalCount 1) -as [string]).Trim() }
function Test-GameDir([string]$dir) {
    return $dir -and (Test-Path -LiteralPath (Join-Path $dir $exeName))
}

# The data folder the launcher uses (its VICTORY_DATA_DIR line).
function Get-SavesDir {
    $dir = $null
    $cmd = $(if ($smallCmd) { $smallCmd } else { $normalCmd })
    if ($cmd) {
        $line = Select-String -Path $cmd -Pattern 'set "VICTORY_DATA_DIR=([^"]+)"' | Select-Object -First 1
        if ($line) { $dir = [Environment]::ExpandEnvironmentVariables($line.Matches[0].Groups[1].Value) }
    }
    if ($env:VICTORY_DATA_DIR) { $dir = $env:VICTORY_DATA_DIR }
    if (-not $dir) { $dir = Join-Path $env:LOCALAPPDATA 'victory\data' }
    return $dir
}

# ------------------------------------------------------------ look
$scale = 1.5
$cheese = [System.Drawing.Color]::FromArgb(181, 230, 85)
$ink = [System.Drawing.Color]::FromArgb(20, 20, 20)
$shade = [System.Drawing.Color]::FromArgb(185, 16, 14, 12)
$titleFont = New-Object System.Drawing.Font('Segoe UI', 16, [System.Drawing.FontStyle]::Bold)
$font = New-Object System.Drawing.Font('Segoe UI', 10)
$bold = New-Object System.Drawing.Font('Segoe UI', 10, [System.Drawing.FontStyle]::Bold)
$small = New-Object System.Drawing.Font('Segoe UI', 8.5)

$form = New-Object System.Windows.Forms.Form
$form.Text = 'VIctory-BigCheese'
$form.FormBorderStyle = 'FixedSingle'
$form.MaximizeBox = $false
$form.StartPosition = 'CenterScreen'
$form.ClientSize = New-Object System.Drawing.Size([int](280 * $scale), [int](504 * $scale))
$form.BackColor = [System.Drawing.Color]::Black
$iconFile = Join-Path $here 'the_big_cheese.ico'
if (Test-Path $iconFile) { try { $form.Icon = New-Object System.Drawing.Icon($iconFile) } catch {} }

# A shortcut with the cheese icon next to BigCheese-Launcher.cmd (a .cmd file
# cannot have an icon of its own), made again at each start so it follows the
# folder if it moves.
$launcherCmd = Join-Path $root 'BigCheese-Launcher.cmd'
if ((Test-Path $launcherCmd) -and (Test-Path $iconFile) -and -not $env:BIGCHEESE_LAUNCHER_SHOT) {
    try {
        $link = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $root 'VIctory-BigCheese.lnk'))
        $link.TargetPath = $launcherCmd
        $link.WorkingDirectory = $root
        $link.IconLocation = "$iconFile,0"
        $link.WindowStyle = 7
        $link.Description = 'VIctory-BigCheese launcher'
        $link.Save()
    } catch {}
}

$picture = Join-Path $here 'the_big_cheese.png'
if (Test-Path $picture) {
    $form.BackgroundImage = [System.Drawing.Image]::FromFile($picture)
    $form.BackgroundImageLayout = 'Stretch'
}
$W = $form.ClientSize.Width

function New-Panel([int]$y, [int]$h) {
    $p = New-Object System.Windows.Forms.Panel
    $p.SetBounds(12, $y, $W - 24, $h)
    $p.BackColor = $shade
    $form.Controls.Add($p)
    return $p
}
function New-Label($parent, [string]$text, [int]$x, [int]$y, [int]$w, [int]$h, $f, $color) {
    $l = New-Object System.Windows.Forms.Label
    $l.Text = $text
    $l.SetBounds($x, $y, $w, $h)
    $l.Font = $f
    $l.ForeColor = $color
    $l.BackColor = [System.Drawing.Color]::Transparent
    $parent.Controls.Add($l)
    return $l
}
function New-Button($parent, [string]$text, [int]$x, [int]$y, [int]$w, [int]$h, [bool]$main) {
    $b = New-Object System.Windows.Forms.Button
    $b.Text = $text
    $b.SetBounds($x, $y, $w, $h)
    $b.FlatStyle = 'Flat'
    $b.Font = $bold
    $b.Cursor = [System.Windows.Forms.Cursors]::Hand
    if ($main) {
        $b.BackColor = $cheese; $b.ForeColor = $ink; $b.FlatAppearance.BorderSize = 0
    } else {
        $b.BackColor = [System.Drawing.Color]::FromArgb(45, 40, 36); $b.ForeColor = [System.Drawing.Color]::White
        $b.FlatAppearance.BorderColor = $cheese
    }
    $parent.Controls.Add($b)
    return $b
}
function New-Check($parent, [string]$text, [string]$note, [int]$y, [bool]$on) {
    $c = New-Object System.Windows.Forms.CheckBox
    $c.Text = $text
    $c.Checked = $on
    $c.SetBounds(14, $y, $parent.Width - 28, 24)
    $c.Font = $bold
    $c.ForeColor = [System.Drawing.Color]::White
    $c.BackColor = [System.Drawing.Color]::Transparent
    $c.Cursor = [System.Windows.Forms.Cursors]::Hand
    $parent.Controls.Add($c)
    if ($note) { $c.Tag = New-Label $parent $note 34 ($y + 23) ($parent.Width - 48) 20 $small ([System.Drawing.Color]::Gainsboro) }
    return $c
}

$title = New-Panel 12 46
[void](New-Label $title 'VIctory-BigCheese' 0 6 $title.Width 34 $titleFont $cheese)
$title.Controls[0].TextAlign = 'MiddleCenter'

# ------------------------------------------------------------ page 1: game folder
$page1 = New-Panel ($form.ClientSize.Height - 232) 220
[void](New-Label $page1 'Game folder' 14 12 200 22 $bold $cheese)
[void](New-Label $page1 "The folder with $exeName in it." 14 34 ($page1.Width - 28) 20 $small ([System.Drawing.Color]::Gainsboro))
$folderBox = New-Object System.Windows.Forms.TextBox
$folderBox.SetBounds(14, 60, $page1.Width - 28, 24)
$folderBox.Font = $font
$folderBox.Text = $gameDir
$page1.Controls.Add($folderBox)
$folderState = New-Label $page1 '' 14 90 ($page1.Width - 28) 40 $small ([System.Drawing.Color]::White)
$browse = New-Button $page1 'Browse...' 14 140 120 34 $false
$next = New-Button $page1 'Next' ($page1.Width - 134) 140 120 34 $true

function Update-Folder {
    $dir = $folderBox.Text.Trim().Trim('"')
    if (Test-GameDir $dir) {
        $folderState.Text = 'Found the game.'
        $folderState.ForeColor = $cheese
        $next.Enabled = $true
    } else {
        $folderState.Text = $(if ($dir) { "There is no $exeName in this folder." } else { 'Choose the folder with Browse.' })
        $folderState.ForeColor = [System.Drawing.Color]::FromArgb(255, 170, 150)
        $next.Enabled = $false
    }
}
$folderBox.Add_TextChanged({ Update-Folder })
$browse.Add_Click({
    $d = New-Object System.Windows.Forms.FolderBrowserDialog
    $d.Description = "Choose your game folder (the folder with $exeName)"
    $d.ShowNewFolderButton = $false
    if ($folderBox.Text -and (Test-Path -LiteralPath $folderBox.Text)) { $d.SelectedPath = $folderBox.Text }
    if ($d.ShowDialog($form) -eq 'OK') { $folderBox.Text = $d.SelectedPath }
})

# ------------------------------------------------------------ page 2: options
$page2 = New-Panel ($form.ClientSize.Height - 392) 380
$page2.Visible = $false
[void](New-Label $page2 'Options' 14 10 200 22 $bold $cheese)
$rt = New-Check $page2 'Ray tracing' 'Reflections and shadows traced; about half the fps in busy streets.' 38 ($settings.rt -eq '1')
$ram16 = New-Check $page2 '16 GB of RAM' 'Memory settings for a PC with 16 GB of RAM.' 88 ($settings.ram16 -eq '1')
$vram8 = New-Check $page2 '8 GB graphics card' 'Card memory sized for 8 GB (a first guess, not measured).' 138 ($settings.vram8 -eq '1')
[void](New-Label $page2 'Console' 14 190 200 22 $bold $cheese)
$seriesX = New-Object System.Windows.Forms.RadioButton
$seriesX.Text = 'Series X (recommended)'
$seriesX.SetBounds(14, 214, $page2.Width - 28, 24)
$seriesS = New-Object System.Windows.Forms.RadioButton
$seriesS.Text = 'Series S (less detail, less memory)'
$seriesS.SetBounds(14, 240, $page2.Width - 28, 24)
foreach ($r in $seriesX, $seriesS) {
    $r.Font = $bold; $r.ForeColor = [System.Drawing.Color]::White
    $r.BackColor = [System.Drawing.Color]::Transparent; $r.Cursor = [System.Windows.Forms.Cursors]::Hand
    $page2.Controls.Add($r)
}
if ($settings.console -eq 'series-s') { $seriesS.Checked = $true } else { $seriesX.Checked = $true }

# Ray tracing with Series X on 16 GB of RAM ran the PC out of memory 40 s
# after the start (29 Nov 2026: Windows' whole commit, RAM and page file,
# used up); with Series S it played. So with both ticked it is off.
$rtNote = $rt.Tag.Text
function Update-RayTracing {
    if ($ram16.Checked -and $seriesX.Checked) {
        $rt.Checked = $false
        $rt.Enabled = $false
        $rt.Tag.Text = 'Not with Series X on 16 GB of RAM (runs out of memory).'
        $rt.Tag.ForeColor = [System.Drawing.Color]::FromArgb(255, 170, 150)
    } else {
        $rt.Enabled = $true
        $rt.Tag.Text = $rtNote
        $rt.Tag.ForeColor = [System.Drawing.Color]::Gainsboro
    }
}
$ram16.Add_CheckedChanged({ Update-RayTracing })
$seriesX.Add_CheckedChanged({ Update-RayTracing })
Update-RayTracing
$saves = New-Button $page2 'Open saves folder' 14 280 ($page2.Width - 28) 32 $false
$back = New-Button $page2 'Back' 14 322 110 44 $false
$play = New-Button $page2 'PLAY' 134 322 ($page2.Width - 148) 44 $true
$play.Font = $titleFont

$next.Add_Click({ $page1.Visible = $false; $page2.Visible = $true })
$back.Add_Click({ $page2.Visible = $false; $page1.Visible = $true })

$saves.Add_Click({
    $data = Get-SavesDir
    $dir = Join-Path $data '7952987A\saves'
    if (-not (Test-Path -LiteralPath $dir)) { $dir = $data }
    if (Test-Path -LiteralPath $dir) {
        Start-Process explorer.exe -ArgumentList "`"$dir`""
    } else {
        [void][System.Windows.Forms.MessageBox]::Show($form, "No saves yet. They will be in:`n$data\7952987A\saves", 'Saves')
    }
})

$play.Add_Click({
    $dir = $folderBox.Text.Trim().Trim('"').TrimEnd('\')
    $cmd = $(if ($ram16.Checked) { $smallCmd } else { $normalCmd })
    if (-not $cmd) {
        [void][System.Windows.Forms.MessageBox]::Show($form, "The .cmd launchers are missing from the Other-Launchers folder. Extract the whole zip again.", 'VIctory-BigCheese')
        return
    }
    $settings.rt = $(if ($rt.Checked) { '1' } else { '0' })
    $settings.ram16 = $(if ($ram16.Checked) { '1' } else { '0' })
    $settings.vram8 = $(if ($vram8.Checked) { '1' } else { '0' })
    $settings.console = $(if ($seriesS.Checked) { 'series-s' } else { 'series-x' })
    Save-Settings

    # Only for the run started here (the .cmd launchers keep settings set before them).
    $env:VICTORY_CONSOLE = $settings.console
    if ($rt.Checked) { $env:VICTORY_BVH_BUILDER = '1' } else { Remove-Item Env:VICTORY_BVH_BUILDER -ErrorAction SilentlyContinue }
    $card8 = @{
        VICTORY_WINDOWS_BAR_MB = '2048'; VICTORY_CARD_BUFFERS_MB = '1024'; VICTORY_INPLACE_MB = '512'
        VICTORY_CARD_ROOM_FOR_TARGETS_MB = '1536'; VICTORY_WINDOW_CARD_PERCENT = '80'
    }
    foreach ($k in $card8.Keys) {
        if ($vram8.Checked) { Set-Item "Env:$k" $card8[$k] } else { Remove-Item "Env:$k" -ErrorAction SilentlyContinue }
    }
    Start-Process -FilePath $env:ComSpec -ArgumentList "/c `"`"$cmd`" `"$dir`"`"" -WorkingDirectory $root
    $form.Close()
})

Update-Folder
if ($env:BIGCHEESE_LAUNCHER_SHOT) {
    # Test only: pictures of both pages instead of the window.
    $form.Add_Shown({
        foreach ($p in 1, 2) {
            $page1.Visible = ($p -eq 1); $page2.Visible = ($p -eq 2); $form.Refresh()
            $bmp = New-Object System.Drawing.Bitmap($form.Width, $form.Height)
            $form.DrawToBitmap($bmp, (New-Object System.Drawing.Rectangle(0, 0, $form.Width, $form.Height)))
            $bmp.Save("$($env:BIGCHEESE_LAUNCHER_SHOT)-page$p.png"); $bmp.Dispose()
        }
        $form.Close()
    })
}
[void]$form.ShowDialog()
