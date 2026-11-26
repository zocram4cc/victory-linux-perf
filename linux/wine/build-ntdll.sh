#!/bin/bash
# Compiles the unix side of Wine's ntdll alone from ./wine into ./obj-ntdll/ntdll.so.
set -u
cd "$(dirname "$0")"
mkdir -p obj-ntdll cfg
# The installed library needs only libgcc_s and libc: no libunwind.
sed 's|^#define HAVE_LIBUNWIND 1|/* #undef HAVE_LIBUNWIND */|' build/include/config.h > cfg/config.h
SRCS=$(tr '\n' ' ' < wine/dlls/ntdll/Makefile.in | sed 's/\\ *//g' | grep -o 'unix/[a-z_0-9]*\.c' | grep -v -E 'signal_(arm|arm64|i386)\.c')
FLAGS="-Icfg -Iwine/dlls/ntdll -Ibuild/include -Iwine/include -D__WINESRC__ -D_NTSYSTEM_ -D_ACRTIMP= -DWINBASEAPI= -DWINE_UNIX_LIB -fPIC -g -O2 -Wall -pipe -fcf-protection=none -fvisibility=hidden -fno-stack-protector -fno-strict-aliasing -Wno-misleading-indentation -fasynchronous-unwind-tables"
LOADER_DEFS='-DBINDIR="/usr/local/bin" -DLIBDIR="/usr/local/lib" -DDATADIR="/usr/local/share" -DSYSTEMDLLPATH=""'
for c in $SRCS; do
  o=obj-ntdll/$(basename ${c%.c}).o
  ( gcc $FLAGS $LOADER_DEFS -c "wine/dlls/ntdll/$c" -o "$o" 2> "${o%.o}.err" || echo "FAILED $c" ) &
done
wait
gcc -Icfg -fPIC -fvisibility=hidden -c cfg/version.c -o obj-ntdll/version.o && gcc -shared -Wl,-Bsymbolic -Wl,-soname,ntdll.so -o obj-ntdll/ntdll.so obj-ntdll/*.o -lm -lpthread -ldl 2> obj-ntdll/link.err && ls -la obj-ntdll/ntdll.so
