@echo off
rem Play-16GB-RAM.cmd for a PC with 16 GB of RAM and an 8 GB graphics card.
rem Play-16GB-RAM.cmd sizes the card's memory for a 16 GB card: 6 GB of game
rem buffers and 4 GB of buffer copies there, more than an 8 GB card holds, so
rem textures had no room (black) or Windows moved card memory into the PC's.
rem Here those are a third to a quarter of that, and less is kept free for
rem render targets. These sizes are a first guess, not measured on an 8 GB
rem card yet: if the game runs out of memory, set VICTORY_WINDOWS_BAR_MB higher
rem before the call line; if textures go black, lower it.
setlocal
if not defined VICTORY_WINDOWS_BAR_MB set "VICTORY_WINDOWS_BAR_MB=2048"
if not defined VICTORY_CARD_BUFFERS_MB set "VICTORY_CARD_BUFFERS_MB=1024"
if not defined VICTORY_INPLACE_MB set "VICTORY_INPLACE_MB=512"
if not defined VICTORY_CARD_ROOM_FOR_TARGETS_MB set "VICTORY_CARD_ROOM_FOR_TARGETS_MB=1536"
if not defined VICTORY_WINDOW_CARD_PERCENT set "VICTORY_WINDOW_CARD_PERCENT=80"
echo Sizes for an 8 GB graphics card.
call "%~dp0Play-16GB-RAM.cmd" %*
