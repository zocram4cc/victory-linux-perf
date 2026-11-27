param(
    [Parameter(Mandatory = $true)]
    [string]$PrefsFile,
    [string]$ConfigFile = "",
    [string]$RootDir = ""
)

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

# --- Detect Hardware (exact 64-bit VRAM via display class registry + OS memory) ---
$gpuNames = @()
$maxVramBytes = [uint64]0
$hasAmd = $false
try {
    $regPath = "HKLM:\SYSTEM\ControlSet001\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\0*"
    Get-ItemProperty -Path $regPath -ErrorAction SilentlyContinue | ForEach-Object {
        $desc = [string]$_.DriverDesc
        if ($desc) {
            $gpuNames += $desc
            if ($desc -match "AMD|Radeon") { $hasAmd = $true }
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
    foreach ($line in Get-Content $ConfigFile -ErrorAction SilentlyContinue) {
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
    foreach ($line in Get-Content $PrefsFile -ErrorAction SilentlyContinue) {
        if ($line -match 'set\s+"XEN_FRAMEGEN=([01])"') { $frameGen = ($Matches[1] -eq "1") }
        if ($line -match 'set\s+"XEN_PROFILE=([^"]+)"') { $profile = $Matches[1].Trim() }
    }
}

# --- Build Dark WinForms Launcher ---
$form = New-Object System.Windows.Forms.Form
$form.Text = "RAZDA4A XEN - Launch Options"
$form.Size = New-Object System.Drawing.Size(520, 345)
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
    $bannerText = "[!] Non-AMD GPU detected ($($gpuNames[0])): this branch targets AMD RDNA GPUs only."
    $bannerColor = [System.Drawing.Color]::FromArgb(248, 196, 113)
} elseif ($isLowVram) {
    $bannerText = "[i] ${vramGb} GB VRAM detected: Safe Mode pre-selected to prevent black screen after menu."
    $bannerColor = [System.Drawing.Color]::FromArgb(133, 193, 233)
} elseif ($freeCommitGb -lt 12.0) {
    $bannerText = "[!] Low free virtual memory (${freeCommitGb} GB): use Low-Commit NVMe or expand pagefile."
    $bannerColor = [System.Drawing.Color]::FromArgb(248, 196, 113)
} elseif ($vramGb -gt 0) {
    $bannerText = "[ok] Detected GPU VRAM: ${vramGb} GB | Free Virtual Memory: ${freeCommitGb} GB"
}

$lblBanner = New-Object System.Windows.Forms.Label
$lblBanner.Text = $bannerText
$lblBanner.ForeColor = $bannerColor
$lblBanner.Font = New-Object System.Drawing.Font("Segoe UI", 8.5)
$lblBanner.Size = New-Object System.Drawing.Size(478, 32)
$lblBanner.Location = New-Object System.Drawing.Point(18, 38)
$form.Controls.Add($lblBanner)

# Executable selector (Browse... writes config.bat automatically)
$lblExe = New-Object System.Windows.Forms.Label
$lblExe.Text = "Target Executable (saved to config.bat):"
$lblExe.AutoSize = $true
$lblExe.Location = New-Object System.Drawing.Point(18, 74)
$form.Controls.Add($lblExe)

$txtExe = New-Object System.Windows.Forms.TextBox
$txtExe.Text = $gameExe
$txtExe.Location = New-Object System.Drawing.Point(18, 96)
$txtExe.Size = New-Object System.Drawing.Size(384, 24)
$txtExe.BackColor = [System.Drawing.Color]::FromArgb(36, 39, 48)
$txtExe.ForeColor = [System.Drawing.Color]::White
$txtExe.BorderStyle = "FixedSingle"
$form.Controls.Add($txtExe)

$btnBrowse = New-Object System.Windows.Forms.Button
$btnBrowse.Text = "Browse..."
$btnBrowse.Size = New-Object System.Drawing.Size(84, 24)
$btnBrowse.Location = New-Object System.Drawing.Point(410, 96)
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
$chkFg.Text = "Enable AMD FidelityFX Frame Generation (2x FPS, HUDLess)"
$chkFg.Checked = $frameGen
$chkFg.AutoSize = $true
$chkFg.Location = New-Object System.Drawing.Point(18, 138)
$form.Controls.Add($chkFg)

$lblFgHint = New-Object System.Windows.Forms.Label
$lblFgHint.Text = "Uncheck for pure native rendering (recommended on 8 GB VRAM or lowest latency)."
$lblFgHint.ForeColor = [System.Drawing.Color]::FromArgb(165, 172, 186)
$lblFgHint.Font = New-Object System.Drawing.Font("Segoe UI", 8.5)
$lblFgHint.AutoSize = $true
$lblFgHint.Location = New-Object System.Drawing.Point(36, 162)
$form.Controls.Add($lblFgHint)

# Profile selector
$lblProf = New-Object System.Windows.Forms.Label
$lblProf.Text = "Memory / GPU Profile:"
$lblProf.AutoSize = $true
$lblProf.Location = New-Object System.Drawing.Point(18, 196)
$form.Controls.Add($lblProf)

$cmbProf = New-Object System.Windows.Forms.ComboBox
$cmbProf.DropDownStyle = "DropDownList"
$cmbProf.Location = New-Object System.Drawing.Point(175, 193)
$cmbProf.Size = New-Object System.Drawing.Size(319, 26)
$cmbProf.BackColor = [System.Drawing.Color]::FromArgb(36, 39, 48)
$cmbProf.ForeColor = [System.Drawing.Color]::White
[void]$cmbProf.Items.Add("Balanced (Default, Auto-VRAM 12-16+ GB)")
[void]$cmbProf.Items.Add("Low-Commit NVMe (16 GB RAM / Small Pagefile)")
[void]$cmbProf.Items.Add("Max FPS Experimental (PORT_CBV)")
[void]$cmbProf.Items.Add("Safe Mode (8-12 GB VRAM / Max Stability)")
$idxMap = @{ "balanced" = 0; "low_commit" = 1; "cbv_exp" = 2; "safe" = 3 }
$cmbProf.SelectedIndex = if ($idxMap.ContainsKey($profile)) { $idxMap[$profile] } else { 0 }
$form.Controls.Add($cmbProf)

$lblNote = New-Object System.Windows.Forms.Label
$lblNote.Text = "Tip: First launch compiles PSO shaders; second launch is much smoother. F2/~ = mouse, F3 = HUD."
$lblNote.ForeColor = [System.Drawing.Color]::FromArgb(145, 152, 168)
$lblNote.Font = New-Object System.Drawing.Font("Segoe UI", 8.2)
$lblNote.AutoSize = $true
$lblNote.Location = New-Object System.Drawing.Point(18, 232)
$form.Controls.Add($lblNote)

$btnLaunch = New-Object System.Windows.Forms.Button
$btnLaunch.Text = "Launch"
$btnLaunch.Size = New-Object System.Drawing.Size(120, 34)
$btnLaunch.Location = New-Object System.Drawing.Point(254, 260)
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
$btnCancel.Location = New-Object System.Drawing.Point(384, 260)
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
    @(
        "@echo off"
        "REM Saved automatically by RAZDA4A XEN launcher"
        "set `"GAME_DIR=$chosenDir`""
        "set `"GAME_EXE=%GAME_DIR%\$exeName`""
    ) | Set-Content -Path $ConfigFile -Encoding ASCII
}

$fgVal = if ($chkFg.Checked) { "1" } else { "0" }
$profKeys = @("balanced", "low_commit", "cbv_exp", "safe")
$profVal = $profKeys[$cmbProf.SelectedIndex]

@(
    "@echo off"
    "set `"XEN_FRAMEGEN=$fgVal`""
    "set `"XEN_PROFILE=$profVal`""
) | Set-Content -Path $PrefsFile -Encoding ASCII

exit 0
