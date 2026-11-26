#!/usr/bin/env bash
# Once a second, while the launcher <pid> lives, appends a row to <dir>/system.csv:
# the title's memory (the Wine process of victory.exe), the system's RAM and
# swap, the card's memory, load, clock and heat, and the CPUs' load.
# Started by play-linux.sh for every game run.
#
#   scripts/victory-sampler.sh <run dir> <launcher pid> <umu-run pid>
set -uo pipefail
dir="$1"
watch="$2"
root="${3:-}"
here="$(dirname "$0")"
out="$dir/system.csv"
echo "t_s,victory_rss_mb,victory_swap_mb,mem_available_mb,swap_used_mb,commit_mb,vram_used_mb,gpu_util_pct,gpu_clock_mhz,gpu_temp_c,cpu_busy_pct,cpu_busiest_core_pct" > "$out"

declare -a prev_total=() prev_idle=()
read_cpus() {
  # Per core (cpuN lines) and the whole (cpu): busy percentages since the last call.
  local i=0 name user nice sys idle iow irq sirq steal rest total idle_all busiest=0 whole=0
  while read -r name user nice sys idle iow irq sirq steal rest; do
    [[ "$name" == cpu* ]] || break
    total=$((user + nice + sys + idle + iow + irq + sirq + steal))
    idle_all=$((idle + iow))
    if [[ -n "${prev_total[$i]:-}" ]]; then
      local dt=$((total - prev_total[i])) di=$((idle_all - prev_idle[i])) pct=0
      (( dt > 0 )) && pct=$(( (100 * (dt - di)) / dt ))
      if [[ "$name" == cpu ]]; then whole=$pct; elif (( pct > busiest )); then busiest=$pct; fi
    fi
    prev_total[$i]=$total
    prev_idle[$i]=$idle_all
    i=$((i + 1))
  done < /proc/stat
  echo "$whole,$busiest"
}

start=$(date +%s)
read_cpus >/dev/null
while kill -0 "$watch" 2>/dev/null; do
  sleep 1
  t=$(( $(date +%s) - start ))
  pid=""
  [[ -n "$root" ]] && pid=$("$here/victory-title-pid.sh" "$root" || true)
  rss=""; swap=""
  if [[ -n "$pid" && -r /proc/$pid/status ]]; then
    rss=$(awk '/^VmRSS:/ {printf "%.0f", $2/1024}' /proc/$pid/status)
    swap=$(awk '/^VmSwap:/ {printf "%.0f", $2/1024}' /proc/$pid/status)
  fi
  mem=$(awk '/^MemAvailable:/ {a=$2} /^SwapTotal:/ {st=$2} /^SwapFree:/ {sf=$2} /^Committed_AS:/ {c=$2}
             END {printf "%.0f,%.0f,%.0f", a/1024, (st-sf)/1024, c/1024}' /proc/meminfo)
  gpu=$(nvidia-smi --query-gpu=memory.used,utilization.gpu,clocks.gr,temperature.gpu \
          --format=csv,noheader,nounits 2>/dev/null | head -1 | tr -d ' ')
  [[ -n "$gpu" ]] || gpu=",,,"
  echo "$t,$rss,$swap,$mem,$gpu,$(read_cpus)" >> "$out"
done
