@echo off
setlocal
rem This file is in the Other-Launchers folder: victory.exe, the fsr and D3D12
rem folders, early-pipelines.txt and game-folder.txt are one folder up.
for %%R in ("%~dp0..") do set "root=%%~fR\"
rem VIctory-BigCheese: checks a GTA VI Xbox 1.0.2.1 game folder: every file the game needs there,
rem at the right size (names and sizes only; nothing is changed or sent).
echo Checking your game folder against the list of files in a working one...
echo.
cd /d "%root%"
rem Dragged onto this file, the folder the launchers remember, next to this
rem file or one folder up, or else asked.
set "GAME_DIR=%~1"
if not defined GAME_DIR if exist "%root%game-folder.txt" set /p GAME_DIR=<"%root%game-folder.txt"
if not defined GAME_DIR if exist "%root%game_scarlett_master.exe" set "GAME_DIR=%root%"
if not defined GAME_DIR if exist "%root%..\game_scarlett_master.exe" for %%G in ("%root%..") do set "GAME_DIR=%%~fG"
if not defined GAME_DIR (
  for /f "usebackq delims=" %%D in (`powershell -NoProfile -STA -Command "Add-Type -AssemblyName System.Windows.Forms; $o = New-Object System.Windows.Forms.Form -Property @{TopMost = $true; ShowInTaskbar = $false}; $d = New-Object System.Windows.Forms.FolderBrowserDialog; $d.Description = 'Choose your game folder (the folder with game_scarlett_master.exe)'; $d.ShowNewFolderButton = $false; if ($d.ShowDialog($o) -eq 'OK') { $d.SelectedPath }; $o.Dispose()" 2^>nul`) do set "GAME_DIR=%%D"
)
if not defined GAME_DIR (
  echo No folder chosen.
  pause
  exit /b 1
)
set "GAME_DIR=%GAME_DIR:"=%"
echo Game folder: %GAME_DIR%
echo.
set "LIST=%~f0"
powershell -NoProfile -Command "$dir = $env:GAME_DIR; $want = Get-Content -LiteralPath $env:LIST | Where-Object { $_ -like 'rem *|*' } | ForEach-Object { $p = $_.Substring(4).Split('|'); [pscustomobject]@{ Path = $p[0]; Size = [int64]$p[1] } }; $bad = 0; foreach ($w in $want) { $f = Join-Path $dir $w.Path; if (-not (Test-Path -LiteralPath $f)) { 'MISSING:    ' + $w.Path; $bad++ } else { $s = (Get-Item -LiteralPath $f).Length; if ($s -ne $w.Size) { 'WRONG SIZE: ' + $w.Path + '  (' + $s + ' bytes, should be ' + $w.Size + ')'; $bad++ } } }; ''; if ($bad -eq 0) { 'All ' + $want.Count + ' game files are there, at the right sizes.' } else { [string]$bad + ' of ' + $want.Count + ' files are missing or the wrong size: copy or download those again.' }"
echo.
echo Send a screenshot of this window.
pause
exit /b 0

rem ---- the files of a working game folder: path, then bytes ----
rem amd_fidelityfx_loader_xbox.dll|13312
rem amd_fidelityfx_upscaler_xbox.dll|786432
rem anim.rpf|4407626384
rem appdata0.rpf|8880
rem appxmanifest.xml|3394
rem audio.rpf|28543454672
rem Bink2Scarlett.dll|255488
rem Catalog000.bin|815104
rem common.rpf|6642192
rem concrt140.dll|298992
rem data.rpf|481544304
rem data\ttdb\graphics\ttdb_graphics.ttpf|34411776
rem data\ttdb\interiors\ttdb_interiors_0_3.ttpf|883202048
rem data\ttdb\interiors\ttdb_interiors_1_3.ttpf|898113536
rem data\ttdb\interiors\ttdb_interiors_2_3.ttpf|881431552
rem data\ttdb\interiors\ttdb_interiors_3_3.ttpf|900875008
rem data\ttdb\lighting\lighting.ttpf|151531776
rem data\ttdb\maps\ttdb_maps_0_7.ttpf|1765679616
rem data\ttdb\maps\ttdb_maps_1_7.ttpf|1746187776
rem data\ttdb\maps\ttdb_maps_2_7.ttpf|1818806528
rem data\ttdb\maps\ttdb_maps_3_7.ttpf|1787349760
rem data\ttdb\maps\ttdb_maps_4_7.ttpf|1791670784
rem data\ttdb\maps\ttdb_maps_5_7.ttpf|1840029440
rem data\ttdb\maps\ttdb_maps_6_7.ttpf|1745923072
rem data\ttdb\maps\ttdb_maps_7_7.ttpf|1755748608
rem data\ttdb\peds\decorations_0_3.ttpf|494316032
rem data\ttdb\peds\decorations_1_3.ttpf|510001152
rem data\ttdb\peds\decorations_2_3.ttpf|491507456
rem data\ttdb\peds\decorations_3_3.ttpf|497391360
rem data\ttdb\peds\peds_loose.ttpf|135676160
rem data\ttdb\peds\ttdb_peds_0_3.ttpf|953116416
rem data\ttdb\peds\ttdb_peds_1_3.ttpf|965556736
rem data\ttdb\peds\ttdb_peds_2_3.ttpf|937699072
rem data\ttdb\peds\ttdb_peds_3_3.ttpf|957291520
rem data\ttdb\props\octa_imposters.ttpf|983620352
rem data\ttdb\props\ttdb_props_0_3.ttpf|1304458240
rem data\ttdb\props\ttdb_props_1_3.ttpf|1256908800
rem data\ttdb\props\ttdb_props_2_3.ttpf|1239990784
rem data\ttdb\props\ttdb_props_3_3.ttpf|1328911616
rem data\ttdb\terrain\ttdb_terrain_0_7.ttpf|2775084288
rem data\ttdb\terrain\ttdb_terrain_1_7.ttpf|2739444480
rem data\ttdb\terrain\ttdb_terrain_2_7.ttpf|2780145920
rem data\ttdb\terrain\ttdb_terrain_3_7.ttpf|2718679808
rem data\ttdb\terrain\ttdb_terrain_4_7.ttpf|2741339648
rem data\ttdb\terrain\ttdb_terrain_5_7.ttpf|2674379008
rem data\ttdb\terrain\ttdb_terrain_6_7.ttpf|2688808960
rem data\ttdb\terrain\ttdb_terrain_7_7.ttpf|2828296704
rem data\ttdb\terrain\ttdb_terrain_image_based_0_1.ttpf|1946070528
rem data\ttdb\terrain\ttdb_terrain_image_based_1_1.ttpf|1890978048
rem data\ttdb\ui\platform.ttpf|222976
rem data\ttdb\ui\ttdb_ui.ttpf|845115648
rem data\ttdb\vehicles\ttdb_vehicles.ttpf|195418624
rem data\ttdb\vehicles\ttdb_vehicles_loose_0_1.ttpf|838474496
rem data\ttdb\vehicles\ttdb_vehicles_loose_1_1.ttpf|890206208
rem data\ttdb\vfx\ttdb_vfx.ttpf|238897664
rem data\ttdb\weapons\ttdb_weapons.ttpf|1421518080
rem data\ttdb\weapons\weapon_liveries.ttpf|739758848
rem game_scarlett_master.exe|120181760
rem gameos.xvd|348069888
rem layout_3202719e-b4b4-421c-9f8f-70d1c4a7d60b_xs.xml|13698
rem levels.rpf|76155065056
rem libHttpClient.GDK.dll|975856
rem microsoft_gameconfig.bin|8192
rem MicrosoftGame.Config|2542
rem movies.rpf|8832481776
rem msvcp140.dll|565232
rem msvcp140_1.dll|26104
rem msvcp140_2.dll|187888
rem msvcp140_atomic_wait.dll|57312
rem msvcp140_codecvt_ids.dll|22008
rem navmeshes.rpf|6707512496
rem pack\core_bvh4\core_bvh4.rpf|8297921520
rem pack\core_licenses\core_licenses_data.rpf|13536
rem pack\core_restofworld\core_restofworld.rpf|1295760752
rem pack\core_restofworld\data\ttdb\peds\ttdb_peds.ttpf|24529408
rem pack\core_restofworld\data\ttdb\props\ttdb_props.ttpf|35286784
rem pack\core_restofworld\data\ttdb\textures_loose.ttpf|21081856
rem pack\core_restofworld\data\ttdb\ui\ttdb_ui.ttpf|4729600
rem pack\core_restofworld\data\ttdb\vfx\ttdb_vfx.ttpf|3129600
rem pack\core_restofworld001\core_restofworld001.rpf|791984
rem pack\core_restofworld001\data\ttdb\ui\ttdb_ui.ttpf|38656
rem pack\shared_minigame000\data\ttdb\maps\ttdb_maps.ttpf|193522176
rem pack\shared_minigame000\data\ttdb\terrain\ttdb_terrain_image_based.ttpf|1427712
rem pack\shared_minigame000\shared_minigame000.rpf|810581008
rem pack\shared_minigame000_bvh4\shared_minigame000_bvh4.rpf|61987040
rem pack\story000\data\ttdb\maps\ttdb_maps.ttpf|697378560
rem pack\story000\data\ttdb\terrain\ttdb_terrain_image_based.ttpf|222333440
rem pack\story000\data\ttdb\vfx\ttdb_vfx.ttpf|3357184
rem pack\story000\story000.rpf|20022508672
rem pack\story000\story000_script.rpf|78023728
rem pack\story000_bvh4\story000_bvh4.rpf|5632
rem pack\story000_restofworld\story000_restofworld.rpf|198354992
rem packs.rpf|4798213264
rem resources.pri|1152
rem script.rpf|4358960
rem shaders.rpf|411695600
rem textures.rpf|192585024
rem Update.alignmentchunk|1
rem vccorlib140.dll|336888
rem vcomp140.dll|72176
rem vcruntime140.dll|97776
rem vcruntime140_1.dll|37848
rem XboxBvhBuilder.dll|884224
rem xbs_logo_150.png|54536
rem xbs_logo_44.png|7102
rem xbs_logo_480.png|404663
rem xbs_logo_store.png|29356
rem xbs_splash.png|2507194
rem XCurl.dll|199152
