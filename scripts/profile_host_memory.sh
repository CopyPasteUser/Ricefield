#!/usr/bin/env bash
set -euo pipefail

# Read-only Linux process sampling: no app startup, settings or filesystem
# mutation. Run RiceField and Thunar with equivalent folders and views.
if (( $# < 1 || $# > 2 )); then
  echo "Usage: $0 <process-pid> [sample-seconds: 1..60]" >&2
  exit 2
fi
pid="$1"
seconds="${2:-10}"
if [[ ! "$pid" =~ ^[1-9][0-9]*$ || ! "$seconds" =~ ^[1-9][0-9]*$ ]] ||
   (( seconds > 60 )); then
  echo 'Expected a positive numeric PID and 1..60 sampling seconds' >&2
  exit 2
fi
proc="/proc/$pid"
for path in "$proc/stat" "$proc/status" "$proc/smaps_rollup" "$proc/comm"; do
  if [[ ! -r "$path" ]]; then
    echo "Cannot sample PID $pid (exited, inaccessible or no smaps_rollup)" >&2
    exit 1
  fi
done
name="$(cat "$proc/comm")"
[[ "$name" =~ ^[[:alnum:]_.-]+$ ]] || name='unknown'
samples="$(mktemp)"
trap 'rm -f -- "$samples"' EXIT
read_ticks() {
  # The two target comm values (ricefield and thunar) do not contain spaces.
  awk '{print $14 + $15}' "$proc/stat"
}
start_ticks="$(read_ticks)"
start_ns="$(date +%s%N)"
for ((i=0; i<=seconds; i++)); do
  if [[ ! -r "$proc/smaps_rollup" ]]; then
    echo "PID $pid vanished during sampling" >&2
    exit 1
  fi
  rss="$(awk '/^VmRSS:/ {print $2}' "$proc/status")"
  threads="$(awk '/^Threads:/ {print $2}' "$proc/status")"
  pss="$(awk '/^Pss:/ {print $2}' "$proc/smaps_rollup")"
  if [[ ! "$rss" =~ ^[0-9]+$ || ! "$pss" =~ ^[0-9]+$ ||
        ! "$threads" =~ ^[0-9]+$ ]]; then
    echo 'Incomplete /proc measurement; not reporting partial data' >&2
    exit 1
  fi
  printf '%s\t%s\t%s\n' "$rss" "$pss" "$threads" >> "$samples"
  if (( i < seconds )); then
    sleep 1
  fi
done
end_ticks="$(read_ticks)"
end_ns="$(date +%s%N)"
hz="$(getconf CLK_TCK)"
echo "process = \"$name\""
echo "pid = $pid"
echo "session = \"${XDG_SESSION_TYPE:-unknown}\""
awk -F '\t' -v begin="$start_ns" -v finish="$end_ns" \
  -v before="$start_ticks" -v after="$end_ticks" -v hz="$hz" '
  NR == 1 {
    rss_min = rss_max = $1;
    pss_min = pss_max = $2;
    threads_min = threads_max = $3;
  }
  {
    rss_total += $1;
    pss_total += $2;
    if ($1 < rss_min) rss_min = $1;
    if ($1 > rss_max) rss_max = $1;
    if ($2 < pss_min) pss_min = $2;
    if ($2 > pss_max) pss_max = $2;
    if ($3 < threads_min) threads_min = $3;
    if ($3 > threads_max) threads_max = $3;
  }
  END {
    duration = (finish - begin) / 1000000000;
    printf "sample_count = %d\n", NR;
    printf "elapsed_seconds = %.3f\n", duration;
    printf "rss_min_kib = %d\n", rss_min;
    printf "rss_max_kib = %d\n", rss_max;
    printf "rss_avg_kib = %.1f\n", rss_total / NR;
    printf "pss_min_kib = %d\n", pss_min;
    printf "pss_max_kib = %d\n", pss_max;
    printf "pss_avg_kib = %.1f\n", pss_total / NR;
    printf "threads_min = %d\n", threads_min;
    printf "threads_max = %d\n", threads_max;
    if (duration > 0 && hz > 0)
      printf "cpu_percent_one_core = %.3f\n", 100 * (after - before) / hz / duration;
  }' "$samples"
