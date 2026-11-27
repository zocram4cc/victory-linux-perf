@echo off
setlocal
REM ============================================================================
REM  PRESET 1: Low-Commit NVMe (16 GB RAM + Tight Pagefile / Background Apps)
REM  Verified in run53: 28.4-30.4 FG FPS, 8.04 GB VRAM, 5.38 GB Shared,
REM  >11 GB free Commit during load spikes via auto-deleting NVMe temp files.
REM ============================================================================
for %%I in ("%~dp0..") do set "ROOT=%%~fI"
if not exist "%ROOT%\config.bat" (
    echo [ERROR] config.bat not found in "%ROOT%"!
    pause
    exit /b 1
)
call "%ROOT%\config.bat"
if not exist "%GAME_EXE%" (
    echo [ERROR] Edit config.bat first! Executable not found: "%GAME_EXE%"
    pause
    exit /b 1
)
if not exist "%ROOT%\cache" mkdir "%ROOT%\cache"
if not exist "%ROOT%\data" mkdir "%ROOT%\data"
for /f "delims==" %%V in ('set VICTORY_ 2^>nul') do set "%%V="

set "VICTORY_GAME_DIR=%GAME_DIR%"
set "VICTORY_CACHE_DIR=%ROOT%\cache"
set "VICTORY_DATA_DIR=%ROOT%\data"
set "VICTORY_FIDELITYFX_DIR=%ROOT%\fidelityfx"
set "VICTORY_LOG=info"
set "VICTORY_FSR_PC=1"
set "VICTORY_AMBIENT=0.75"
set "VICTORY_AMBIENT_AIR=1.5"
set "VICTORY_AMBIENT_REFLECT=1.0"
set "VICTORY_SCALAR_SPLITS=0"
set "VICTORY_FIDELITYFX_FRAMEGEN=1"
set "VICTORY_FRAMEGEN_AHEAD=1"
set "VICTORY_FRAMEGEN_HUDLESS=1"
set "VICTORY_ALLOW_TEARING=1"
set "VICTORY_VSYNC=0"
set "VICTORY_FRAME_INTERVAL_MS=16.667"
set "VICTORY_COMPUTE_WAIT_MS=5000"
set "VICTORY_HELPER_PRIORITY=1"
set "VICTORY_SUBMIT_FRAMES=2"
set "VICTORY_CARD_STEPS=2"
set "VICTORY_PLACED_ORDER=1"
set "VICTORY_FETCH_CHECK=1"
set "VICTORY_TEXTURE_NO_WAIT=1"
set "VICTORY_RELAY=2"
set "VICTORY_WIDE_BACK=1"
set "VICTORY_WIDE_SPIN=100"
set "VICTORY_CARD_REHASH_RUNS=1"
set "VICTORY_JOIN_COPIES=1"
set "VICTORY_JOIN_BARRIERS=1"
set "VICTORY_UP_QUEUE=1"
set "VICTORY_INDIRECT_WINDOW=1"
set "VICTORY_RELAY_COPY=1"
set "VICTORY_INDIRECT_BATCH=2"
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
set "VICTORY_LIBRARY=1"
set "VICTORY_LIBRARY_STORE_LATER=1"
set "VICTORY_LIBRARY_SAVE_S=60"
set "VICTORY_PIPELINE_BOOK=1"
set "VICTORY_PIPELINE_WORKERS=4"
set "VICTORY_RESIDENCY=1"
set "VICTORY_SEGMENT_TIGHT=92"
set "VICTORY_SWEEP_ASYNC=1"
set "VICTORY_STAGING_IDLE_MS=1500"
set "VICTORY_WINDOW_IDLE_MS=2500"
set "VICTORY_SCRATCH_SPARE_MB=16"
set "VICTORY_READ_KEEP_MB=16"
set "VICTORY_WAIT_RESIDENT=textures"
set "VICTORY_CARD_BUFFERS_MIN_KB=64"
set "VICTORY_KEEP_FREED=0"
set "VICTORY_LIST_IDLE_MOST=1024"
set "VICTORY_LIST_IDLE_TRIM_S=60"
set "VICTORY_LATE_WAIT_LISTS=160"
set "VICTORY_BURY_LAG=4"
set "VICTORY_SNAPSHOT_POOL=32"
set "VICTORY_MEMORY_MAP=0"
REM Key difference: forced file-backed physical pages on fast NVMe (%LOCALAPPDATA%\victory\physical)
set "VICTORY_FILE_PHYSICAL=1"
set "VICTORY_STAGING_BAR=0"
set "VICTORY_SEND_BAR=0"
set "VICTORY_SKIP_PIPELINES=240cb516c1af0bc3"
set "VICTORY_KBM_PC=1"
set "VICTORY_PANEL=1"
set "VICTORY_PANEL_POS=top-right"

echo ============================================================================
echo  RAZDA4A XEN — Preset: Low-Commit NVMe (16 GB RAM / Tight Pagefile)
echo  NVMe File-Backed Physical Pages ^| ~8.04 GB VRAM ^| HUDLess FG + Tearing
echo ============================================================================
pushd "%ROOT%"
start "" /WAIT /B /ABOVENORMAL "%ROOT%\RAZDA4A_XEN.exe" run --console series-s "%GAME_EXE%" --execute
popd
pause
