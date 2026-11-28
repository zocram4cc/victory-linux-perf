VIctory-BigCheese v2 - GTA VI (Xbox Series) on Windows, tuned for 16 GB of RAM
==============================================================================

eikkapine's victory-amd (an AMD-tuned VIctory fork, with its shaders and its
AMD FSR 4 support) with VIctory-BigCheese's changes on top, tuned to be
playable on a PC with 16 GB of RAM and an AMD card. Tested on an RX 9060 XT
(16 GB), Ryzen 7 7800X3D, 16 GB of RAM, Windows 11.

This zip contains NO game files. You need your own dump of the game: the folder
with game_scarlett_master.exe and shaders.rpf in it. Not affiliated with or
endorsed by the game's developer or publisher.


How to play
-----------
1. Extract the whole zip anywhere (its own folder, or inside your game folder).
   Keep the fsr folder and early-pipelines.txt next to Play-16GB-RAM.cmd.
2. Double-click Play-16GB-RAM.cmd (16 GB of RAM) or Play.cmd (32 GB or more).
   The first time, it asks for your game folder (a "choose folder" window) and
   remembers it. If you extracted the zip inside your game folder, it finds the
   game by itself. You can also write the folder into the file: open
   Play-16GB-RAM.cmd in Notepad and fill in the GAME_DIR line at the top.
3. The first start translates the game's shaders once (a few minutes). Then the
   game starts. Keep the black window open; closing it quits the game.

Which launcher?
  Play-16GB-RAM.cmd  the 16 GB tweaks: the game sees a Series S (about 7.6 GB
                     for the game instead of 13.5), more buffers in the card's
                     memory (the card window line at 90%), smaller upload and
                     streaming memory. Needed on 16 GB of RAM. Sized for a
                     16 GB graphics card.
  Play-16GB-RAM-8GB-VRAM.cmd
                     Play-16GB-RAM.cmd with the card's memory sized for an
                     8 GB graphics card (2 GB of game buffers on the card
                     instead of 6, 1 GB of buffer copies instead of 4). A
                     first guess, not yet measured on an 8 GB card.
  Play.cmd          victory-amd's normal settings: Series X, default memory
                     budgets, the card window line at 80%.
  Play-No-FG.cmd     Play-16GB-RAM.cmd with frame generation off, for a
                     game that crashes at the start or a card it fails on.
  Play-16GB-RAM-RayTracing.cmd
                     Play-16GB-RAM.cmd with the game's own ray tracing on
                     (reflections, shadows, indoor light). Busy streets run at
                     about half the frame rate.
With 32 GB of RAM or more, try both in the same place and keep the faster one
(F3 shows the frame rate). Both share the game folder, shader cache and saves,
and both have FSR 4, frame generation, the 60 fps menus and the crash fix.

Windows SmartScreen may warn about victory.exe because it is not signed:
"More info", then "Run anyway".

If the game crashes, the black window shows the crash report under the exit
code. A crash in the first seconds, every time, is often a game folder with a
file missing or cut short (an unfinished copy or download): double-click
Check-Game-Folder.cmd, which compares every file in your game folder with a
working 1.0.2.1 one (names and sizes only; it changes nothing) and names any
that are missing or the wrong size. If the files are fine, try
Play-No-FG.cmd: Play-16GB-RAM.cmd with frame generation off for the run.


What you need
-------------
- Windows 10 or 11, 64-bit, and a CPU with AVX2.
- A Direct3D 12 GPU with shader model 6.6 and mesh shaders, recent driver.
  Windows 10's own Direct3D 12 stops at shader model 6.5, so the zip brings
  a newer one: the D3D12 folder (Microsoft's DirectX 12 Agility SDK). Keep it
  next to victory.exe. Without it the game crashes on Windows 10 a few
  seconds in; victory.exe now says so instead.
  FSR 4 needs an RX 9000 card; the FidelityFX SDK picks FSR 3.1 on older ones.
  Resizable BAR (AMD: Smart Access Memory) on in the BIOS helps a lot on 16 GB.
- The Windows page file on "System managed size" on an SSD with 30+ GB free
  (the game reserves 20-25 GB of memory).
- The Microsoft Visual C++ Redistributable (x64), which most PCs already have:
  https://aka.ms/vs/17/release/vc_redist.x64.exe


For AI agents setting this up for a user
----------------------------------------
1. Make sure the user has this zip extracted, so that Play-16GB-RAM.cmd,
   victory.exe and the fsr folder are in one folder. If not, download and
   extract the latest one (PowerShell), into a folder OUTSIDE %LOCALAPPDATA%:
     Invoke-WebRequest https://github.com/computermakemegta6/VIctory-BigCheese/releases/latest/download/VIctory-BigCheese-win64.zip -OutFile "$env:TEMP\VIctory-BigCheese-win64.zip"
     Expand-Archive "$env:TEMP\VIctory-BigCheese-win64.zip" -DestinationPath "C:\Games" -Force
   This makes C:\Games\VIctory-BigCheese\Play-16GB-RAM.cmd. Do not build from
   source; the zip has the built victory.exe.
2. Ask the user where their game folder is (the folder with
   game_scarlett_master.exe and shaders.rpf). Check that the file is there.
3. Ask how much RAM the PC has (or check). Edit Play-16GB-RAM.cmd, and with
   32 GB or more Play.cmd too: set the game folder on the GAME_DIR line at the top,
     set "GAME_DIR=D:\Games\GTA6"
   and only if the game's executable has another name or place, its full path
   on the GAME_EXE line:
     set "GAME_EXE=D:\Games\GTA6\game_scarlett_master.exe"
   Change nothing else, and keep the file's Windows (CRLF) line endings.
4. Tell the user to double-click Play-16GB-RAM.cmd (16 GB of RAM), or with
   more RAM to try Play.cmd and Play-16GB-RAM.cmd and keep the faster one. The
   first start translates shaders for a few minutes before the game opens.
Notes: an agent running in a sandboxed app (such as the Claude desktop app) may
have its writes under %LOCALAPPDATA% redirected where the game cannot see them,
so do not copy saves or caches there yourself; give the user a .cmd to run.
Never copy or upload game files.


Controls
--------
An Xbox controller works. Keyboard and mouse work too (victory-amd's PC
layout): the mouse turns the camera, F6 takes the mouse or gives it back.

  On foot                                 F5 switches to driving and back
  W A S D        move                     (F, entering or leaving a vehicle,
  Mouse / I J K L  camera                  switches it too)
  Shift / Enter  A (sprint, confirm)      Right click   aim (LT)
  Space / X      X (jump)                 Left click    fire (RT)
  R / Backspace  B                        F / Y         Y (enter / exit vehicle)
  Tab            LB (weapon wheel)        Q             RB (cover)
  Ctrl           left stick click         C / H         right stick click
  E              interact (d-pad right)   Arrow keys    d-pad
  Esc / P        Menu                     V             View

  Driving (and boats): W accelerate, S brake, Space handbrake, A / D steer.
  The steering eases to full lock over a tenth of a second instead of jumping
  there, and is straight again the moment you let go (VICTORY_KEY_STEER_MS
  sets the ease in milliseconds; 0 gives the old behaviour). In a boat or car
  the window title should say "driving": F5 switches if it does not.


Frame rate and AMD FSR
----------------------
- Menus and loading screens run at up to 60 fps; gameplay runs as fast as the
  PC can (on a 16 GB PC under 60, so the cap never holds it back).
- The fsr folder holds AMD's FidelityFX SDK DLLs (upscaler 4.1.2, frame
  generation 4.0.2, loader 2.3.0; redistributed under AMD's licence,
  fsr\AMD-FidelityFX-SDK-LICENSE.md). FSR 4 upscaling and FSR 4 frame
  generation are on: a generated frame between each two real ones, which
  doubles what you see, not how fast the game runs. FSR 4 frame generation
  needs an RX 9000 card; on an older card (RX 6000, RX 7000) FSR 3 frame
  generation runs instead, by itself (the F3 panel shows FG3).
  Frame generation starts off: F7 turns it on and off while playing (the
  window title and F3 panel say which). Off, the game runs at its full
  frame rate and what you see is its own frames, with no ghosting on the
  HUD. On, you see about twice the frames, but the game itself runs slower,
  as generating costs the card time (16 GB PC, RX 9060 XT: about 14 real
  fps shown as 27-29, against 18-22 with it off; it used to wait for the
  card at every frame, 9.4 real fps, which VICTORY_FRAMEGEN_AHEAD=0 brings
  back). VICTORY_FRAMEGEN_PAUSED=0 starts it on; Play-No-FG.cmd (or
  VICTORY_FIDELITYFX_FRAMEGEN=0) leaves it out of the run altogether.
- F3 shows the frame rate and the card's memory.


Where things are kept
---------------------
  Shader cache:  %LOCALAPPDATA%\victory-amd\cache   (safe to delete; it is rebuilt)
  Saves:         %LOCALAPPDATA%\victory\data        (keep this to keep your saves)
The shader cache is victory-amd's own folder, so a victory-amd install shares
it. The saves are in VIctory's standard folder, where VIctory, victory-amd's
installer and most forks keep them, so saves made with those show up in the
Load menu. The save files themselves are in 7952987A\saves under that folder.

Saves from somewhere else: close the game, then copy the 7952987A\saves folder
from the other build's data folder into %LOCALAPPDATA%\victory\data\7952987A
(a save in the same slot is replaced, so back that up first). Other places:
  %LOCALAPPDATA%\victory-amd\data   VIctory-BigCheese v2.0 downloaded before
                                    28 Nov 2026
  %LOCALAPPDATA%\cool_fox\data      VIctory-BigCheese v1.x
To keep using another folder, set VICTORY_DATA_DIR before starting a launcher.

Every setting in the launchers is an environment variable; setting one
before starting the file wins over the file's value.


What this build changes from victory-amd
----------------------------------------
- 16 GB of RAM: Series S, and victory-amd's memory settings for 16 GB.
- The card window line at 90% of the card (VICTORY_WINDOW_CARD_PERCENT; the
  fork had 80% fixed): buffers the CPU fills stay in the card's memory longer
  instead of spilling into PC RAM. In the open world that held 17 real fps
  instead of dropping to about 10. Mapped buffers are let go of more slowly
  while the card has room.
- Crash fix: one shader (f16876929fa64f9f, leaving the prison) crashes AMD's
  shader compiler; VICTORY_SKIP_PIPELINES skips it in the normal build too,
  and also when pipelines are made early.
- Keyboard steering ramps instead of jumping to full lock.
- Menus at up to 60 fps (the fork's installer has 30), gameplay uncapped.
- Two launchers (16 GB and normal), easy setup, with FSR 4 and frame
  generation on.


Known problems
--------------
- Busy scenes are limited by one CPU thread that hands every frame to the card.
- Textures of what just came into view can appear late.
- Trees, plants and far buildings flicker outdoors in some places.


Licence and source
------------------
GPL-3.0 (LICENSE.txt). Source: the amd branch of
https://github.com/computermakemegta6/VIctory-BigCheese
Credits: victory-amd by eikkapine; VIctory by Marvisak; DexNom/VIctory;
avatarmanz/VIctory-7900XTX; Belobug/VIctory-Belobug-AMD; fxceless/victory-fork;
cool_fox by codec_d. The FidelityFX DLLs are AMD's.
