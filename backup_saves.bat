@echo off
setlocal
set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"
set "SRC=%ROOT%\data"
set "DST_ROOT=%ROOT%\saves-backup"
set "DST_LATEST=%DST_ROOT%\latest"

if not exist "%SRC%" (
    echo [ERROR] Data directory not found yet: %SRC%
    echo Run the game at least once first.
    pause
    exit /b 1
)

echo ============================================================================
echo  Save Backup Utility — RAZDA4A XEN
echo ============================================================================
echo  [1] Backup %SRC% -^> %DST_LATEST% (+ timestamped copy)
echo  [2] Restore %DST_LATEST% -^> %SRC%
echo  [Q] Quit
echo.
set /p "CHOICE=Select [1/2/Q]: "
if /I "%CHOICE%"=="1" goto :backup
if /I "%CHOICE%"=="2" goto :restore
exit /b 0

:backup
for /f %%I in ('powershell -NoProfile -Command "Get-Date -Format yyyyMMdd-HHmmss"') do set "STAMP=%%I"
robocopy "%SRC%" "%DST_LATEST%" /MIR /R:1 /W:1 /NFL /NDL /NP
robocopy "%SRC%" "%DST_ROOT%\snapshot-%STAMP%" /E /R:1 /W:1 /NFL /NDL /NP
echo [OK] Backup completed: %DST_LATEST%
pause
exit /b 0

:restore
if not exist "%DST_LATEST%" (
    echo [ERROR] No backup found at %DST_LATEST%
    pause
    exit /b 1
)
robocopy "%DST_LATEST%" "%SRC%" /MIR /R:1 /W:1 /NFL /NDL /NP
echo [OK] Restored from %DST_LATEST%
pause
exit /b 0
