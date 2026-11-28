#!/bin/bash
set -u
root=$(cd "$(dirname "$0")/.." && pwd)
server=$root/deps/wineserver/server/wineserver
if [ "${VICTORY_WINE_VIEW_TREE:-1}" != 0 ] && [ -x "$server" ]; then
  if [ "$(wine --version)" = "$(cat "$root/deps/wineserver/version" 2>/dev/null)" ]; then
    mount --bind "$server" "$(command -v wineserver)" || exit 1
  else
    echo "the system wine changed: run $root/setup.sh wine to get the patched wineserver back" >&2
  fi
fi
trap 'timeout -k 1s 4s wineserver -k >/dev/null 2>&1 || true' EXIT
wine "$@"
