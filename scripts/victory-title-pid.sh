#!/usr/bin/env bash
# Prints the PID of the title's Wine process among the descendants of <root>
# (the umu-run play-linux.sh started), or nothing. Under umu the process is
# several levels down, in a session of pressure-vessel's own, and its name is
# not victory.exe (Wine names its main thread); its first argument is.
#
#   scripts/victory-title-pid.sh <root pid>
set -uo pipefail
descend() {
  local child
  for child in $(pgrep -P "$1" 2>/dev/null); do
    echo "$child"
    descend "$child"
  done
}
for pid in $(descend "${1:?root pid}"); do
  # A process that has ended meanwhile is passed over quietly (the redirect's
  # own error is the shell's, hence the braces).
  first=$( { tr '\0' '\n' < "/proc/$pid/cmdline"; } 2>/dev/null | head -n 1)
  case "${first,,}" in
    *\\victory.exe | */victory.exe | victory.exe) echo "$pid"; exit 0 ;;
  esac
done
exit 1
