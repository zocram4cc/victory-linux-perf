@echo off
chcp 65001 >nul
setlocal
set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"
call "%ROOT%\config.bat"
if not exist "%GAME_EXE%" (
    echo [ERROR] Edit config.bat first! Not found: "%GAME_EXE%"
    pause
    exit /b 1
)
echo ============================================================================
echo  Pre-warming used pipelines — RAZDA4A XEN (6 workers)
echo  Make sure RAZDA4A_XEN.exe is NOT running!
echo ============================================================================
set "VICTORY_GAME_DIR=%GAME_DIR%"
set "VICTORY_CACHE_DIR=%ROOT%\cache"
set "VICTORY_DATA_DIR=%ROOT%\data"
set "VICTORY_SCALAR_SPLITS=0"
set "VICTORY_SKIP_PIPELINES=240cb516c1af0bc3"
set "VICTORY_LIBRARY=1"
"%ROOT%\RAZDA4A_XEN.exe" warm --used --jobs 6
pause
