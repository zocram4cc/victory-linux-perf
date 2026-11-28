param(
    [Parameter(Mandatory = $true)]
    [string]$PrefsFile,
    [string]$ConfigFile = "",
    [string]$RootDir = ""
)

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)

# --- Detect Hardware (exact 64-bit VRAM via display class registry + OS memory) ---
$gpuNames = @()
$maxVramBytes = [uint64]0
$hasAmd = $false
$hasRdna1OrOlder = $false
try {
    $regPath = "HKLM:\SYSTEM\ControlSet001\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\0*"
    Get-ItemProperty -Path $regPath -ErrorAction SilentlyContinue | ForEach-Object {
        $desc = [string]$_.DriverDesc
        if ($desc) {
            $gpuNames += $desc
            if ($desc -match "AMD|Radeon") {
                $hasAmd = $true
                if ($desc -match "RX\s*5[0-9]{3}|RX\s*[45][0-9]{2}|Vega|R9\s|R7\s") {
                    $hasRdna1OrOlder = $true
                }
            }
            $mem = $_.{"HardwareInformation.qwMemorySize"}
            if ($null -eq $mem) { $mem = $_.{"HardwareInformation.MemorySize"} }
            if ($mem -is [byte[]] -and $mem.Length -ge 4) {
                $mem = [BitConverter]::ToUInt32($mem, 0)
            }
            if ($null -ne $mem) {
                $v = [uint64]$mem
                if ($v -gt $maxVramBytes) { $maxVramBytes = $v }
            }
        }
    }
} catch {}

$vramGb = if ($maxVramBytes -gt 0) { [math]::Round($maxVramBytes / 1GB, 1) } else { 0 }
$isVeryLowVram = ($vramGb -gt 0 -and $vramGb -lt 5.5)
$is6GbVram = ($vramGb -ge 5.5 -and $vramGb -le 6.5)
$isLowVram = ($vramGb -gt 0 -and $vramGb -le 8.5)
$freeCommitGb = 99.0
try {
    $os = Get-CimInstance Win32_OperatingSystem -ErrorAction SilentlyContinue
    if ($os) {
        $freeCommitGb = [math]::Round(($os.FreeVirtualMemory) / 1MB, 1)
    }
} catch {}

# --- Load current executable path from config.bat ---
$gameDir = ""
$gameExe = ""
if ($ConfigFile -and (Test-Path $ConfigFile)) {
    foreach ($line in Get-Content $ConfigFile -Encoding UTF8 -ErrorAction SilentlyContinue) {
        if ($line -match '^\s*set\s+"GAME_DIR=([^"]*)"') { $gameDir = $Matches[1] }
        if ($line -match '^\s*set\s+"GAME_EXE=([^"]*)"') { $gameExe = $Matches[1] }
    }
    if ($gameDir -and $gameExe) {
        $gameExe = $gameExe.Replace("%GAME_DIR%", $gameDir)
    }
}

# --- Load saved preferences (or apply smart hardware defaults on first run) ---
$hasSavedPrefs = Test-Path $PrefsFile
$frameGen = -not $isLowVram
$profile = if ($isLowVram) { "safe" } else { "balanced" }

if ($hasSavedPrefs) {
    foreach ($line in Get-Content $PrefsFile -Encoding UTF8 -ErrorAction SilentlyContinue) {
        if ($line -match 'set\s+"XEN_FRAMEGEN=([01])"') { $frameGen = ($Matches[1] -eq "1") }
        if ($line -match 'set\s+"XEN_PROFILE=([^"]+)"') { $profile = $Matches[1].Trim() }
    }
}

# --- Build Dark WinForms Launcher ---
$form = New-Object System.Windows.Forms.Form
$form.Text = "RAZDA4A XEN - Launch Options"
$form.Size = New-Object System.Drawing.Size(540, 365)
$form.StartPosition = "CenterScreen"
$form.FormBorderStyle = "FixedDialog"
$form.MaximizeBox = $false
$form.MinimizeBox = $false
$form.TopMost = $true
$form.BackColor = [System.Drawing.Color]::FromArgb(24, 26, 32)
$form.ForeColor = [System.Drawing.Color]::FromArgb(235, 238, 245)
$form.Font = New-Object System.Drawing.Font("Segoe UI", 9.5)

$title = New-Object System.Windows.Forms.Label
$title.Text = "RAZDA4A XEN (16 GB RAM / Win10 Edition)"
$title.Font = New-Object System.Drawing.Font("Segoe UI", 11, [System.Drawing.FontStyle]::Bold)
$title.AutoSize = $true
$title.Location = New-Object System.Drawing.Point(18, 12)
$form.Controls.Add($title)

# Hardware / preflight notice banner
$bannerText = ""
$bannerColor = [System.Drawing.Color]::FromArgb(130, 224, 170)
if ($gpuNames.Count -gt 0 -and -not $hasAmd) {
    $bannerText = "[!] Non-AMD GPU detected ($($gpuNames[0])): AMD RDNA2+ only. For NVIDIA GPUs use trustno1-coder/VIctory-NVIDIA."
    $bannerColor = [System.Drawing.Color]::FromArgb(248, 196, 113)
} elseif ($hasRdna1OrOlder -and $vramGb -le 8.5) {
    $bannerText = "[!] Older AMD GPU detected ($($gpuNames[0])): DX12 Ultimate Mesh Shaders (RDNA2 RX 6000+) are required."
    $bannerColor = [System.Drawing.Color]::FromArgb(248, 196, 113)
} elseif ($isVeryLowVram) {
    $bannerText = "[!] ${vramGb} GB VRAM detected: below 8 GB minimum (4 GB / 2 GB GPUs will likely OOM or show black screen)."
    $bannerColor = [System.Drawing.Color]::FromArgb(241, 148, 138)
} elseif ($is6GbVram) {
    $bannerText = "[!] ${vramGb} GB VRAM detected: experimental / rough on 6 GB. Use Safe Mode with Frame Gen OFF."
    $bannerColor = [System.Drawing.Color]::FromArgb(248, 196, 113)
} elseif ($isLowVram) {
    $bannerText = "[i] ${vramGb} GB VRAM detected: Safe Mode (65% / 2048 MB / 768 MB) recommended for smooth 8 GB VRAM play."
    $bannerColor = [System.Drawing.Color]::FromArgb(133, 193, 233)
} elseif ($freeCommitGb -lt 12.0) {
    $bannerText = "[!] Low free virtual memory (${freeCommitGb} GB): use Low-Commit NVMe or run pagefile_16gb_ram.ps1."
    $bannerColor = [System.Drawing.Color]::FromArgb(248, 196, 113)
} elseif ($vramGb -gt 0) {
    $bannerText = "[ok] Detected GPU VRAM: ${vramGb} GB | Free Virtual Memory: ${freeCommitGb} GB"
}

$lblBanner = New-Object System.Windows.Forms.Label
$lblBanner.Text = $bannerText
$lblBanner.ForeColor = $bannerColor
$lblBanner.Font = New-Object System.Drawing.Font("Segoe UI", 8.5)
$lblBanner.Size = New-Object System.Drawing.Size(500, 34)
$lblBanner.Location = New-Object System.Drawing.Point(18, 38)
$form.Controls.Add($lblBanner)

# Executable selector (Browse... writes config.bat automatically in UTF-8)
$lblExe = New-Object System.Windows.Forms.Label
$lblExe.Text = "Target Executable (saved to config.bat):"
$lblExe.AutoSize = $true
$lblExe.Location = New-Object System.Drawing.Point(18, 78)
$form.Controls.Add($lblExe)

$txtExe = New-Object System.Windows.Forms.TextBox
$txtExe.Text = $gameExe
$txtExe.Location = New-Object System.Drawing.Point(18, 100)
$txtExe.Size = New-Object System.Drawing.Size(404, 24)
$txtExe.BackColor = [System.Drawing.Color]::FromArgb(36, 39, 48)
$txtExe.ForeColor = [System.Drawing.Color]::White
$txtExe.BorderStyle = "FixedSingle"
$form.Controls.Add($txtExe)

$btnBrowse = New-Object System.Windows.Forms.Button
$btnBrowse.Text = "Browse..."
$btnBrowse.Size = New-Object System.Drawing.Size(84, 24)
$btnBrowse.Location = New-Object System.Drawing.Point(430, 100)
$btnBrowse.BackColor = [System.Drawing.Color]::FromArgb(52, 56, 68)
$btnBrowse.ForeColor = [System.Drawing.Color]::White
$btnBrowse.FlatStyle = "Flat"
$btnBrowse.FlatAppearance.BorderSize = 0
$btnBrowse.Add_Click({
    $dlg = New-Object System.Windows.Forms.OpenFileDialog
    $dlg.Title = "Select your target .exe file"
    $dlg.Filter = "Executable files (*.exe)|*.exe|All files (*.*)|*.*"
    if ($txtExe.Text -and (Test-Path (Split-Path $txtExe.Text -Parent))) {
        $dlg.InitialDirectory = Split-Path $txtExe.Text -Parent
    } elseif ($RootDir -and (Test-Path $RootDir)) {
        $dlg.InitialDirectory = $RootDir
    }
    if ($dlg.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {
        $txtExe.Text = $dlg.FileName
    }
})
$form.Controls.Add($btnBrowse)

# Frame Generation checkbox
$chkFg = New-Object System.Windows.Forms.CheckBox
$chkFg.Text = "Enable AMD FidelityFX Frame Generation (2x Displayed FPS, HUDLess)"
$chkFg.Checked = $frameGen
$chkFg.AutoSize = $true
$chkFg.Location = New-Object System.Drawing.Point(18, 142)
$form.Controls.Add($chkFg)

$lblFgHint = New-Object System.Windows.Forms.Label
$lblFgHint.Text = "Uncheck for pure native rendering (lowest latency; recommended for testing/8 GB troubleshooting)."
$lblFgHint.ForeColor = [System.Drawing.Color]::FromArgb(165, 172, 186)
$lblFgHint.Font = New-Object System.Drawing.Font("Segoe UI", 8.2)
$lblFgHint.AutoSize = $true
$lblFgHint.Location = New-Object System.Drawing.Point(36, 166)
$form.Controls.Add($lblFgHint)

# Profile selector
$lblProf = New-Object System.Windows.Forms.Label
$lblProf.Text = "Memory / GPU Profile:"
$lblProf.AutoSize = $true
$lblProf.Location = New-Object System.Drawing.Point(18, 200)
$form.Controls.Add($lblProf)

$cmbProf = New-Object System.Windows.Forms.ComboBox
$cmbProf.DropDownStyle = "DropDownList"
$cmbProf.Location = New-Object System.Drawing.Point(175, 197)
$cmbProf.Size = New-Object System.Drawing.Size(339, 26)
$cmbProf.BackColor = [System.Drawing.Color]::FromArgb(36, 39, 48)
$cmbProf.ForeColor = [System.Drawing.Color]::White
[void]$cmbProf.Items.Add("Balanced (Default, Auto-VRAM 12-16+ GB)")
[void]$cmbProf.Items.Add("Low-Commit NVMe (16 GB RAM / Small Pagefile)")
[void]$cmbProf.Items.Add("Max FPS Experimental (PORT_CBV)")
[void]$cmbProf.Items.Add("Safe Mode (8-12 GB VRAM / Live-Budget Fit)")
$idxMap = @{ "balanced" = 0; "low_commit" = 1; "cbv_exp" = 2; "safe" = 3 }
$cmbProf.SelectedIndex = if ($idxMap.ContainsKey($profile)) { $idxMap[$profile] } else { 0 }
$form.Controls.Add($cmbProf)

$lblNote = New-Object System.Windows.Forms.Label
$lblNote.Text = "Tip: 1st launch compiles PSO shaders (1-3 min); 2nd launch is smoother. F2/~ = mouse, F3 = HUD."
$lblNote.ForeColor = [System.Drawing.Color]::FromArgb(145, 152, 168)
$lblNote.Font = New-Object System.Drawing.Font("Segoe UI", 8.2)
$lblNote.AutoSize = $true
$lblNote.Location = New-Object System.Drawing.Point(18, 236)
$form.Controls.Add($lblNote)

# Utility buttons (left side): Warm Shaders + Export Bug Report
$btnWarm = New-Object System.Windows.Forms.Button
$btnWarm.Text = "Warm Shaders"
$btnWarm.Size = New-Object System.Drawing.Size(112, 34)
$btnWarm.Location = New-Object System.Drawing.Point(18, 272)
$btnWarm.BackColor = [System.Drawing.Color]::FromArgb(52, 56, 68)
$btnWarm.ForeColor = [System.Drawing.Color]::White
$btnWarm.FlatStyle = "Flat"
$btnWarm.FlatAppearance.BorderSize = 0
$btnWarm.Add_Click({
    $warmBat = Join-Path $RootDir "warm_shaders.bat"
    if (Test-Path $warmBat) {
        Start-Process -FilePath "cmd.exe" -ArgumentList "/c `"$warmBat`"" -WorkingDirectory $RootDir
    }
})
$form.Controls.Add($btnWarm)

$btnLogs = New-Object System.Windows.Forms.Button
$btnLogs.Text = "Export Logs"
$btnLogs.Size = New-Object System.Drawing.Size(102, 34)
$btnLogs.Location = New-Object System.Drawing.Point(138, 272)
$btnLogs.BackColor = [System.Drawing.Color]::FromArgb(52, 56, 68)
$btnLogs.ForeColor = [System.Drawing.Color]::White
$btnLogs.FlatStyle = "Flat"
$btnLogs.FlatAppearance.BorderSize = 0
$btnLogs.Add_Click({
    $logsBat = Join-Path $RootDir "collect_logs.bat"
    if (Test-Path $logsBat) {
        Start-Process -FilePath "cmd.exe" -ArgumentList "/c `"$logsBat`"" -WorkingDirectory $RootDir
    }
})
$form.Controls.Add($btnLogs)

# Primary action buttons (right side)
$btnLaunch = New-Object System.Windows.Forms.Button
$btnLaunch.Text = "Launch"
$btnLaunch.Size = New-Object System.Drawing.Size(120, 34)
$btnLaunch.Location = New-Object System.Drawing.Point(274, 272)
$btnLaunch.BackColor = [System.Drawing.Color]::FromArgb(46, 134, 222)
$btnLaunch.ForeColor = [System.Drawing.Color]::White
$btnLaunch.FlatStyle = "Flat"
$btnLaunch.FlatAppearance.BorderSize = 0
$btnLaunch.Add_Click({
    $chosen = $txtExe.Text.Trim()
    if (-not $chosen -or -not (Test-Path $chosen -PathType Leaf)) {
        [System.Windows.Forms.MessageBox]::Show(
            "Target executable not found.`nClick 'Browse...' and select your unpacked .exe file first.",
            "RAZDA4A XEN",
            [System.Windows.Forms.MessageBoxButtons]::OK,
            [System.Windows.Forms.MessageBoxIcon]::Warning
        ) | Out-Null
        return
    }
    $form.DialogResult = [System.Windows.Forms.DialogResult]::OK
    $form.Close()
})
$form.Controls.Add($btnLaunch)

$btnCancel = New-Object System.Windows.Forms.Button
$btnCancel.Text = "Cancel"
$btnCancel.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
$btnCancel.Size = New-Object System.Drawing.Size(110, 34)
$btnCancel.Location = New-Object System.Drawing.Point(404, 272)
$btnCancel.BackColor = [System.Drawing.Color]::FromArgb(52, 56, 68)
$btnCancel.ForeColor = [System.Drawing.Color]::White
$btnCancel.FlatStyle = "Flat"
$btnCancel.FlatAppearance.BorderSize = 0
$form.Controls.Add($btnCancel)

$form.AcceptButton = $btnLaunch
$form.CancelButton = $btnCancel

$result = $form.ShowDialog()
if ($result -ne [System.Windows.Forms.DialogResult]::OK) {
    exit 2
}

$chosenExe = $txtExe.Text.Trim()
if ($ConfigFile -and $chosenExe -and (Test-Path $chosenExe -PathType Leaf)) {
    $chosenDir = Split-Path $chosenExe -Parent
    $exeName = Split-Path $chosenExe -Leaf
    $cfgLines = @(
        "@echo off"
        "REM Saved automatically by RAZDA4A XEN launcher (UTF-8)"
        "set `"GAME_DIR=$chosenDir`""
        "set `"GAME_EXE=%GAME_DIR%\$exeName`""
    ) -join "`r`n"
    [System.IO.File]::WriteAllText($ConfigFile, $cfgLines + "`r`n", $utf8NoBom)
}

$fgVal = if ($chkFg.Checked) { "1" } else { "0" }
$profKeys = @("balanced", "low_commit", "cbv_exp", "safe")
$profVal = $profKeys[$cmbProf.SelectedIndex]

$prefLines = @(
    "@echo off"
    "set `"XEN_FRAMEGEN=$fgVal`""
    "set `"XEN_PROFILE=$profVal`""
) -join "`r`n"
[System.IO.File]::WriteAllText($PrefsFile, $prefLines + "`r`n", $utf8NoBom)

exit 0
