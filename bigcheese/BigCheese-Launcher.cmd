@echo off
rem Opens the launcher window: game folder, then ray tracing, 16 GB of RAM, an
rem 8 GB card, Series X or S, the saves folder and Play. It starts one of the
rem Play .cmd files next to it with those settings.
start "" powershell -NoProfile -STA -ExecutionPolicy Bypass -WindowStyle Hidden -File "%~dp0launcher\launcher.ps1"
