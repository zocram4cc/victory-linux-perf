@echo off
chcp 65001 >nul
setlocal

REM ============================================================================
REM  RAZDA4A XEN — Sanitized Bug Report & Log Collector
REM  Collects hardware specs (CPU, GPU, VRAM, RAM, OS, Pagefile), launcher
REM  profile, binary SHA-256, and the last 250 lines of logs\last_run.log.
REM  Personal usernames and local game paths are redacted automatically.
REM ============================================================================

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"
if not exist "%ROOT%\logs" mkdir "%ROOT%\logs"

powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$root = '%ROOT%';" ^
  "$ts = Get-Date -Format 'yyyyMMdd_HHmmss';" ^
  "$out = Join-Path $root ('logs\bug_report_' + $ts + '.txt');" ^
  "$sb = New-Object System.Text.StringBuilder;" ^
  "[void]$sb.AppendLine('=== RAZDA4A XEN SANITIZED BUG REPORT ===');" ^
  "[void]$sb.AppendLine('Generated: ' + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss K'));" ^
  "$exe = Join-Path $root 'RAZDA4A_XEN.exe';" ^
  "if (Test-Path $exe) { $h = (Get-FileHash $exe -Algorithm SHA256).Hash; [void]$sb.AppendLine('Binary SHA-256: ' + $h) };" ^
  "try { $os = Get-CimInstance Win32_OperatingSystem; [void]$sb.AppendLine(('OS: {0} (Build {1})' -f $os.Caption, $os.BuildNumber)); [void]$sb.AppendLine(('RAM: {0:N1} GB total | Free Virtual: {1:N1} GB' -f ($os.TotalVisibleMemorySize/1MB), ($os.FreeVirtualMemory/1MB))) } catch {};" ^
  "try { $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1; [void]$sb.AppendLine(('CPU: {0} ({1}C/{2}T)' -f $cpu.Name.Trim(), $cpu.NumberOfCores, $cpu.NumberOfLogicalProcessors)) } catch {};" ^
  "try { $reg = 'HKLM:\SYSTEM\ControlSet001\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}\0*'; Get-ItemProperty $reg -ErrorAction SilentlyContinue | Where-Object { $_.DriverDesc } | ForEach-Object { $m = $_.'HardwareInformation.qwMemorySize'; if ($null -eq $m) { $m = $_.'HardwareInformation.MemorySize' }; if ($m -is [byte[]] -and $m.Length -ge 4) { $m = [BitConverter]::ToUInt32($m,0) }; $gb = if ($m) { [math]::Round([uint64]$m / 1GB, 1) } else { 0 }; [void]$sb.AppendLine(('GPU: {0} | VRAM: {1} GB | Driver: {2}' -f $_.DriverDesc, $gb, $_.DriverVersion)) } } catch {};" ^
  "try { $pf = Get-CimInstance Win32_PageFileUsage -ErrorAction SilentlyContinue; foreach ($p in $pf) { [void]$sb.AppendLine(('Pagefile: Allocated={0} MB Peak={1} MB' -f $p.AllocatedBaseSize, $p.PeakUsage)) } } catch {};" ^
  "$prefs = Join-Path $root 'launcher_prefs.bat';" ^
  "if (Test-Path $prefs) { [void]$sb.AppendLine(''); [void]$sb.AppendLine('--- launcher_prefs.bat ---'); Get-Content $prefs | ForEach-Object { [void]$sb.AppendLine($_) } };" ^
  "$log = Join-Path $root 'logs\last_run.log';" ^
  "if (Test-Path $log) { [void]$sb.AppendLine(''); [void]$sb.AppendLine('--- logs/last_run.log (last 250 lines, paths redacted) ---'); $user = [regex]::Escape($env:USERNAME); Get-Content $log -Tail 250 -ErrorAction SilentlyContinue | ForEach-Object { $l = $_ -replace ('(?i)([A-Z]:\\Users\\)' + $user), '$1<USER>'; [void]$sb.AppendLine($l) } } else { [void]$sb.AppendLine(''); [void]$sb.AppendLine('[No logs/last_run.log found yet — launch once via RAZDA4A XEN.bat or a preset first.]') };" ^
  "[System.IO.File]::WriteAllText($out, $sb.ToString(), (New-Object System.Text.UTF8Encoding($false)));" ^
  "Write-Host ('Saved sanitized bug report to: ' + $out);" ^
  "Start-Process explorer.exe ('/select,\"' + $out + '\"')"

echo.
echo Attach the generated logs\bug_report_*.txt file to a GitHub Issue at:
echo   https://github.com/crossducking/RAZDA4A-XEN/issues
pause
