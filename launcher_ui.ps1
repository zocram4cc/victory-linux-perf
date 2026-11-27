param(
    [Parameter(Mandatory = $true)]
    [string]$PrefsFile
)

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

$frameGen = $true
$profile = "balanced"

if (Test-Path $PrefsFile) {
    foreach ($line in Get-Content $PrefsFile -ErrorAction SilentlyContinue) {
        if ($line -match 'set\s+"XEN_FRAMEGEN=([01])"') { $frameGen = ($Matches[1] -eq "1") }
        if ($line -match 'set\s+"XEN_PROFILE=([^"]+)"') { $profile = $Matches[1].Trim() }
    }
}

$form = New-Object System.Windows.Forms.Form
$form.Text = "RAZDA4A XEN — Launch Options"
$form.Size = New-Object System.Drawing.Size(440, 235)
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
$title.Location = New-Object System.Drawing.Point(18, 14)
$form.Controls.Add($title)

$chkFg = New-Object System.Windows.Forms.CheckBox
$chkFg.Text = "Enable AMD FidelityFX Frame Generation (2x FPS, HUDLess)"
$chkFg.Checked = $frameGen
$chkFg.AutoSize = $true
$chkFg.Location = New-Object System.Drawing.Point(20, 50)
$form.Controls.Add($chkFg)

$lblFgHint = New-Object System.Windows.Forms.Label
$lblFgHint.Text = "Uncheck for pure native rendering (lowest input latency)."
$lblFgHint.ForeColor = [System.Drawing.Color]::FromArgb(165, 172, 186)
$lblFgHint.Font = New-Object System.Drawing.Font("Segoe UI", 8.5)
$lblFgHint.AutoSize = $true
$lblFgHint.Location = New-Object System.Drawing.Point(38, 74)
$form.Controls.Add($lblFgHint)

$lblProf = New-Object System.Windows.Forms.Label
$lblProf.Text = "Memory / GPU Profile:"
$lblProf.AutoSize = $true
$lblProf.Location = New-Object System.Drawing.Point(20, 106)
$form.Controls.Add($lblProf)

$cmbProf = New-Object System.Windows.Forms.ComboBox
$cmbProf.DropDownStyle = "DropDownList"
$cmbProf.Location = New-Object System.Drawing.Point(165, 103)
$cmbProf.Size = New-Object System.Drawing.Size(242, 26)
$cmbProf.BackColor = [System.Drawing.Color]::FromArgb(36, 39, 48)
$cmbProf.ForeColor = [System.Drawing.Color]::White
[void]$cmbProf.Items.Add("Balanced (Default, Auto-VRAM)")
[void]$cmbProf.Items.Add("Low-Commit NVMe (16 GB RAM)")
[void]$cmbProf.Items.Add("Max FPS Experimental (PORT_CBV)")
[void]$cmbProf.Items.Add("Safe Mode (8-12 GB VRAM)")
$idxMap = @{ "balanced" = 0; "low_commit" = 1; "cbv_exp" = 2; "safe" = 3 }
$cmbProf.SelectedIndex = if ($idxMap.ContainsKey($profile)) { $idxMap[$profile] } else { 0 }
$form.Controls.Add($cmbProf)

$btnLaunch = New-Object System.Windows.Forms.Button
$btnLaunch.Text = "Launch"
$btnLaunch.DialogResult = [System.Windows.Forms.DialogResult]::OK
$btnLaunch.Size = New-Object System.Drawing.Size(115, 32)
$btnLaunch.Location = New-Object System.Drawing.Point(172, 150)
$btnLaunch.BackColor = [System.Drawing.Color]::FromArgb(46, 134, 222)
$btnLaunch.ForeColor = [System.Drawing.Color]::White
$btnLaunch.FlatStyle = "Flat"
$btnLaunch.FlatAppearance.BorderSize = 0
$form.Controls.Add($btnLaunch)

$btnCancel = New-Object System.Windows.Forms.Button
$btnCancel.Text = "Cancel"
$btnCancel.DialogResult = [System.Windows.Forms.DialogResult]::Cancel
$btnCancel.Size = New-Object System.Drawing.Size(105, 32)
$btnCancel.Location = New-Object System.Drawing.Point(300, 150)
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

$fgVal = if ($chkFg.Checked) { "1" } else { "0" }
$profKeys = @("balanced", "low_commit", "cbv_exp", "safe")
$profVal = $profKeys[$cmbProf.SelectedIndex]

@(
    "@echo off"
    "set `"XEN_FRAMEGEN=$fgVal`""
    "set `"XEN_PROFILE=$profVal`""
) | Set-Content -Path $PrefsFile -Encoding ASCII

exit 0
