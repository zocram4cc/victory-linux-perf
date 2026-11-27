# Optional helper for 16 GB RAM PCs: sets C:\pagefile.sys to 16-32 GB (Run as Administrator)
$ErrorActionPreference = 'Stop'
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "[!] Please run this script as Administrator." -ForegroundColor Yellow
    pause
    exit 1
}
$cs = Get-CimInstance Win32_ComputerSystem
if ($cs.AutomaticManagedPagefile) {
    Set-CimInstance -InputObject $cs -Property @{ AutomaticManagedPagefile = $false }
}
$pf = Get-CimInstance Win32_PageFileSetting -Filter "Name='C:\\pagefile.sys'"
if ($pf) {
    Set-CimInstance -InputObject $pf -Property @{ InitialSize = 16384; MaximumSize = 32768 }
} else {
    New-CimInstance -ClassName Win32_PageFileSetting -Property @{ Name = 'C:\pagefile.sys'; InitialSize = 16384; MaximumSize = 32768 } | Out-Null
}
Write-Host "[OK] C:\pagefile.sys configured to 16384-32768 MB. Reboot Windows for it to take effect." -ForegroundColor Green
pause
