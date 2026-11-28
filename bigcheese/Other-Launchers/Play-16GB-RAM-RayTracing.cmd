@echo off
rem The 16 GB launcher with the game's own ray tracing on: the game builds
rem its ray-tracing structures, so reflections, shadows and indoor light are
rem traced as on the Xbox. Everything else is the same as Play-16GB-RAM.cmd.
rem Measured once on this PC (test D, 10 minutes, no crash): quiet places
rem about the same, busy streets about half the frame rate (6.5 against
rem 14-17 fps before frame generation).
set "VICTORY_BVH_BUILDER=1"
call "%~dp0Play-16GB-RAM.cmd" %*
