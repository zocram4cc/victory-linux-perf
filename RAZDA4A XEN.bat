@echo off
chcp 65001 >nul
setlocal

REM ============================================================================
REM  RAZDA4A XEN — Budget & 16 GB RAM Edition
REM  Portable x86-64-v3 release build | Win10 Agility SDK | FSR 4.1.2 ML + HUDLess FG
REM  Run with --no-gui to skip the Launch Options dialog (VILauncher / CLI friendly).
REM ============================================================================

set "ROOT=%~dp0"
if "%ROOT:~-1%"=="\" set "ROOT=%ROOT:~0,-1%"

set "XEN_FRAMEGEN=1"
set "XEN_PROFILE=balanced"
if exist "%ROOT%\launcher_prefs.bat" call "%ROOT%\launcher_prefs.bat"

if /I not "%~1"=="--no-gui" (
    if exist "%ROOT%\launcher_ui.ps1" (
        powershell -NoProfile -ExecutionPolicy Bypass -STA -File "%ROOT%\launcher_ui.ps1" -PrefsFile "%ROOT%\launcher_prefs.bat" -ConfigFile "%ROOT%\config.bat" -RootDir "%ROOT%"
        if errorlevel 2 exit /b 0
        if exist "%ROOT%\launcher_prefs.bat" call "%ROOT%\launcher_prefs.bat"
    )
)

if not exist "%ROOT%\config.bat" (
    echo [ERROR] config.bat not found next to RAZDA4A XEN.bat!
    pause
    exit /b 1
)
call "%ROOT%\config.bat"

if not exist "%GAME_EXE%" (
    echo ============================================================================
    echo  [!] Executable not found: "%GAME_EXE%"
    echo  Run RAZDA4A XEN.bat without --no-gui to select your .exe via Browse...,
    echo  or edit config.bat and set GAME_DIR and GAME_EXE to your unpacked folder.
    echo ============================================================================
    pause
    exit /b 1
)

if not exist "%ROOT%\cache" mkdir "%ROOT%\cache"
if not exist "%ROOT%\data" mkdir "%ROOT%\data"
if not exist "%ROOT%\logs" mkdir "%ROOT%\logs"

REM Clear any inherited VICTORY_* variables
for /f "delims==" %%V in ('set VICTORY_ 2^>nul') do set "%%V="

set "VICTORY_GAME_DIR=%GAME_DIR%"
set "VICTORY_CACHE_DIR=%ROOT%\cache"
set "VICTORY_DATA_DIR=%ROOT%\data"
set "VICTORY_FIDELITYFX_DIR=%ROOT%\fidelityfx"
set "VICTORY_LOG=info"
set "VICTORY_LOG_FILE=%ROOT%\logs\last_run.log"

REM === GRAPHICS QUALITY: Native AMD FSR 4.1.2 ML + Reflections + Air ===
set "VICTORY_FSR_PC=1"
set "VICTORY_AMBIENT=0.75"
set "VICTORY_AMBIENT_AIR=1.5"
set "VICTORY_AMBIENT_REFLECT=1.0"
set "VICTORY_SCALAR_SPLITS=0"

REM === AMD FIDELITYFX FRAME GENERATION (chosen in launcher checkbox) ===
set "VICTORY_FIDELITYFX_FRAMEGEN=%XEN_FRAMEGEN%"
set "VICTORY_FRAMEGEN_AHEAD=1"
set "VICTORY_FRAMEGEN_HUDLESS=1"
set "VICTORY_ALLOW_TEARING=1"
set "VICTORY_VSYNC=0"
set "VICTORY_FRAME_INTERVAL_MS=16.667"

REM === FACE FIX + MAX CPU FPS (6-core friendly, fast texture recheck) ===
set "VICTORY_COMPUTE_WAIT_MS=5000"
set "VICTORY_COMPUTE_LATE_WAIT_MS=400"
set "VICTORY_HELPER_PRIORITY=1"
set "VICTORY_SUBMIT_FRAMES=2"
set "VICTORY_CARD_STEPS=2"
set "VICTORY_PLACED_ORDER=1"
set "VICTORY_FETCH_CHECK=1"
set "VICTORY_TEXTURE_NO_WAIT=1"
set "VICTORY_TEXTURE_LOOK_MS=350"
set "VICTORY_RELAY=2"
set "VICTORY_WIDE_BACK=1"
set "VICTORY_WIDE_SPIN=100"
set "VICTORY_CARD_REHASH_RUNS=1"
set "VICTORY_JOIN_COPIES=1"
set "VICTORY_JOIN_BARRIERS=1"
set "VICTORY_UP_QUEUE=1"
set "VICTORY_INDIRECT_WINDOW=0"
set "VICTORY_RELAY_COPY=1"
set "VICTORY_INDIRECT_BATCH=1"
set "VICTORY_MAP_THREADS=1"
set "VICTORY_CARD_AWAKE=1"
set "VICTORY_SHOW_AFTER=1"
set "VICTORY_KEEP_SHOWN=2"
set "VICTORY_TEXTURE_AWAY_MS=10000"
set "VICTORY_THIN=1"
set "VICTORY_THIN_THREADS=4"
set "VICTORY_LAND_THREADS=3"
set "VICTORY_LAND_SKIP=1"
set "VICTORY_JOIN_PARTS=16"
set "VICTORY_UPLOAD_AHEAD=1"
set "VICTORY_HOLES_ONCE=30"
set "VICTORY_FILE_CACHE=1"
set "VICTORY_WC_NO_BACK=1"
set "VICTORY_CLOSED_PLAN=1"
set "VICTORY_PORT_JOURNAL=1"
set "VICTORY_PORT_GUARD=1"
set "VICTORY_PORT_DESCRIPTORS=1"
set "VICTORY_SWEEP_ASYNC=1"

REM === PSO PIPELINE CACHE (4 general + 4 compute/screen workers) ===
set "VICTORY_LIBRARY=1"
set "VICTORY_LIBRARY_STORE_LATER=1"
set "VICTORY_LIBRARY_SAVE_S=60"
set "VICTORY_PIPELINE_BOOK=1"
set "VICTORY_PIPELINE_WORKERS=4"

REM === 16 GB RAM + ZERO-OOM (0x8007000E) + DYNAMIC AUTO-VRAM ===
REM Note: VICTORY_CARD_BUFFERS_MIN_KB is left unset ("auto") so 8 GB GPUs
REM automatically use 512 KB instead of being overridden down to 64 KB.
set "VICTORY_RESIDENCY=1"
set "VICTORY_SEGMENT_TIGHT=92"
set "VICTORY_STAGING_IDLE_MS=1500"
set "VICTORY_WINDOW_IDLE_MS=2500"
set "VICTORY_SCRATCH_SPARE_MB=16"
set "VICTORY_READ_KEEP_MB=16"
set "VICTORY_WAIT_RESIDENT=textures"
set "VICTORY_KEEP_FREED=0"
set "VICTORY_LIST_IDLE_MOST=1024"
set "VICTORY_LIST_IDLE_TRIM_S=60"
set "VICTORY_LATE_WAIT_LISTS=160"
set "VICTORY_BURY_LAG=4"
set "VICTORY_SNAPSHOT_POOL=32"
set "VICTORY_MEMORY_MAP=0"
set "VICTORY_FILE_PHYSICAL=auto"
set "VICTORY_STAGING_BAR=0"
set "VICTORY_SEND_BAR=0"

REM === APPLY SELECTED LAUNCHER PROFILE ===
if /I "%XEN_PROFILE%"=="low_commit" (
    set "VICTORY_FILE_PHYSICAL=1"
)
if /I "%XEN_PROFILE%"=="cbv_exp" (
    set "VICTORY_PORT_CBV=1"
    set "VICTORY_PORT_CBV_PLACE=1"
    set "VICTORY_PORT_CBV_SHADOW=2"
)
if /I "%XEN_PROFILE%"=="safe" (
    REM Verified 8 GB VRAM live-budget fit (PR #1 by nathanbzrr: RX 7600 8 GB 3.67 -> 12.7 native / 24.86 FG FPS)
    set "VICTORY_ALLOW_TEARING=0"
    set "VICTORY_VSYNC=1"
    set "VICTORY_HELPER_PRIORITY=0"
    set "VICTORY_SUBMIT_FRAMES=1"
    set "VICTORY_JOIN_PARTS=12"
    set "VICTORY_PORT_DESCRIPTORS=0"
    set "VICTORY_PIPELINE_WORKERS=2"
    set "VICTORY_SEGMENT_TIGHT=85"
    set "VICTORY_TEXTURE_KEEP_PERCENT=40"
    set "VICTORY_TEXTURE_ROOM_PERCENT=65"
    set "VICTORY_TEXTURE_STAGE_MB=64"
    set "VICTORY_STAGING_MAX_MB=96"
    set "VICTORY_STAGING_IDLE_MS=1000"
    set "VICTORY_WINDOW_BUDGET_MB=96"
    set "VICTORY_WINDOW_IDLE_MS=2000"
    set "VICTORY_SEND_SPARE_MB=24"
    set "VICTORY_CARD_BUFFERS_MIN_KB=128"
    set "VICTORY_CARD_BUFFERS_MB=768"
    set "VICTORY_CARD_ROOM_FOR_TARGETS_MB=2048"
    set "VICTORY_LIST_IDLE_MOST=128"
    set "VICTORY_LIST_IDLE_TRIM_S=30"
    set "VICTORY_LATE_WAIT_LISTS=128"
    set "VICTORY_SNAPSHOT_POOL=24"
    set "VICTORY_FILE_PHYSICAL=1"
)

REM RDNA4 crash-shader shield
set "VICTORY_SKIP_PIPELINES=240cb516c1af0bc3"

REM PC Controls & Perf Overlay (F2 or ~ = mouse-look, F3 = FPS HUD, top-right corner)
set "VICTORY_KBM_PC=1"
set "VICTORY_PANEL=1"
set "VICTORY_PANEL_POS=top-right"

set "FG_LABEL=ON (HUDLess 2x FPS)"
if "%XEN_FRAMEGEN%"=="0" set "FG_LABEL=OFF (Native Zero-Latency)"

echo ============================================================================
echo  RAZDA4A XEN — Budget ^& 16 GB RAM Edition
echo  Profile: %XEN_PROFILE% ^| Frame Generation: %FG_LABEL%
echo  Controls: F2 or ~ = toggle mouse-look ^| F3 = toggle FPS HUD
echo  Tip: Run collect_logs.bat to export a sanitized bug report if needed.
echo ============================================================================
pushd "%ROOT%"
start "" /WAIT /B "%ROOT%\RAZDA4A_XEN.exe" run --console series-s "%GAME_EXE%" --execute
popd
pause
