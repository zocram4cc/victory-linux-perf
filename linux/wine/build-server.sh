#!/bin/bash
# Compiles Wine's server alone from ./wine into ./obj/wineserver.
set -u
cd "$(dirname "$0")"
mkdir -p obj
SRCS=$(tr '\n' ' ' < wine/server/Makefile.in | sed 's/.*SOURCES = \\//; s/UNIX_CFLAGS.*//' | tr -d '\\' | tr ' \t' '\n\n' | grep '\.c$')
FLAGS="-Ibuild/include -Iwine/include -Iwine/server -D__WINESRC__ -DBINDIR=\"/usr/local/bin\" -DDATADIR=\"/usr/local/share\" -g -O2 -Wall -pipe -fcf-protection=none -fvisibility=hidden -fno-stack-protector -fno-strict-aliasing -Wno-misleading-indentation"
fail=0
for c in $SRCS; do
  ( gcc $FLAGS -c "wine/server/$c" -o "obj/${c%.c}.o" 2> "obj/${c%.c}.err" || echo "FAILED $c" ) &
done
wait
grep -l 'error' obj/*.err 2>/dev/null | head
gcc -o obj/wineserver obj/*.o $(grep -E '^(RT_LIBS|INOTIFY_LIBS|PROCSTAT_LIBS) ?=' build/config.status | sed 's/.*= *//' | tr '\n' ' ') -lm && ls -la obj/wineserver
