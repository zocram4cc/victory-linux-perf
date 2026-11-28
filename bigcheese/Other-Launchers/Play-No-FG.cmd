@echo off
rem Play-16GB-RAM.cmd with FidelityFX frame generation off for the whole run
rem (FSR upscaling stays on). Try it if the game crashes in its first seconds,
rem or if frame generation does not work on your card. With 32 GB of RAM or
rem more you can write Play.cmd instead of Play-16GB-RAM.cmd on the last line.
set "VICTORY_FIDELITYFX_FRAMEGEN=0"
echo Frame generation is off for this run.
call "%~dp0Play-16GB-RAM.cmd" %*
