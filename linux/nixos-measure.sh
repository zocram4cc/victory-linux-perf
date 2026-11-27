#!/usr/bin/env bash
# nixos-run.sh, measured: SETTLE s after the world, per-thread CPU (and PERF=1: a perf profile) over WIN s,
# then the run ends. Usage: nixos-measure.sh <name> [nixos-run.sh args after the name and seconds...]
set -uo pipefail
NAME="$1"; shift
T=~/.cache/scarlettport-autotest; OUT=$T/$NAME; SETTLE=${SETTLE:-30}; WIN=${WIN:-15}
"$(dirname "$0")/nixos-run.sh" "$NAME" "${DUR:-200}" "$@" > $T/$NAME.out 2>&1 &
RUN=$!
sleep 15
until [ "$(sed 's/\x1b\[[0-9;]*m//g' $OUT/run.log 2>/dev/null | grep -a 'fps, ' | grep -aoE '[3-9][0-9]{3,} draws/frame|[1-9][0-9]{4,} draws/frame' | wc -l)" -ge 2 ] || ! kill -0 $RUN 2>/dev/null; do sleep 3; done
echo "world $(date +%T)"; sleep "$SETTLE"
PID=$(for p in $(pgrep -f '^Z:.*victory\.exe run'); do echo "$(ls /proc/$p/task | wc -l) $p"; done | sort -rn | awk 'NR==1{print $2}')
snap() { for t in /proc/$PID/task/*; do s=$(cat $t/stat 2>/dev/null) || continue; s=${s##*) }; set -- $s; echo "${t##*/} ${12} ${13} $(cat $t/comm | tr ' ' '_')"; done; }
snap > $OUT/s0
ls /proc/$PID/task | while read t; do echo "$t $(cat /proc/$PID/task/$t/comm)"; done > $OUT/threads.txt
grep -E "victory\.exe|x86_64-windows/ntdll\.dll" /proc/$PID/maps > $OUT/maps.txt
if [ "${PERF:-0}" = 1 ]; then
    nix-shell -p perf --run "perf record -F 999 -p $PID -o $OUT/perf.data -- sleep $WIN" > $OUT/perf.log 2>&1
else
    sleep "$WIN"
fi
snap > $OUT/s1
join <(sort $OUT/s0) <(sort $OUT/s1) | awk -v w=$WIN '{u=($5-$2)/w; k=($6-$3)/w; if (u+k>5) printf "%-8s %-16s user %5.1f%% kernel %5.1f%%\n", $1, $4, u, k}' | sort -k4 -rn > $OUT/threads-cpu.txt
echo "window done $(date +%T)"; head -12 $OUT/threads-cpu.txt
TP=$(pgrep -f "^timeout --signal=INT --kill-after=10 [0-9]*s .*gamescope" | head -1); [ -n "$TP" ] && kill -INT $TP
wait $RUN
sed 's/\x1b\[[0-9;]*m//g' $OUT/run.log | grep -a "fps, " | grep -o "^[^ ]*\|[0-9.]* fps, [0-9.]* ms/frame\|[0-9]* draws/frame" | paste - - - | cut -c12-19,28- | tail -8
