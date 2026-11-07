#!/usr/bin/env bash
set -euo pipefail

# Run inside an isolated Xvfb display. These are lab measurements, not Mint
# hardware acceptance. No settings, thumbnails or journal enter the real home.
binary="${1:-target/release/ricefield}"
output="${2:-performance-desktop.toml}"
fixture="$(mktemp -d)"
app_pid=''
cleanup() {
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  rm -r -- "$fixture"
}
trap cleanup EXIT
mkdir -p "$fixture/files" "$fixture/config" "$fixture/state" "$fixture/cache"
printf 'Desktop lab fixture\n' > "$fixture/files/readme.txt"
start_ns="$(date +%s%N)"
XDG_CONFIG_HOME="$fixture/config" XDG_STATE_HOME="$fixture/state" \
  XDG_CACHE_HOME="$fixture/cache" LIBGL_ALWAYS_SOFTWARE=1 GSETTINGS_BACKEND=memory \
  "$binary" --separate-window "$fixture/files" > "$fixture/app.log" 2>&1 &
app_pid=$!
window=''
for ((attempt = 0; attempt < 200; attempt++)); do
  kill -0 "$app_pid" 2>/dev/null || { cat "$fixture/app.log" >&2; exit 1; }
  window="$(xdotool search --onlyvisible --pid "$app_pid" --name '^RiceField$' | head -n 1 || true)"
  [[ -z "$window" ]] || break
  sleep 0.05
done
[[ -n "$window" ]] || { echo 'No window within ten seconds' >&2; exit 1; }
mapped_ns="$(date +%s%N)"
# Allow startup/initial scans to settle before sampling ten idle seconds.
sleep 3
ticks_before="$(awk '{print $14 + $15}' "/proc/$app_pid/stat")"
idle_start_ns="$(date +%s%N)"
sleep 10
kill -0 "$app_pid"
idle_end_ns="$(date +%s%N)"
ticks_after="$(awk '{print $14 + $15}' "/proc/$app_pid/stat")"
rss_kib="$(awk '/^VmRSS:/ {print $2}' "/proc/$app_pid/status")"
clock_ticks="$(getconf CLK_TCK)"
awk -v version="$("$binary" --version)" -v start="$start_ns" -v mapped="$mapped_ns" \
  -v first="$ticks_before" -v last="$ticks_after" -v hz="$clock_ticks" \
  -v idle_start="$idle_start_ns" -v idle_end="$idle_end_ns" -v rss="$rss_kib" \
  'BEGIN {
    printf "version = \"%s\"\n", version;
    print "display = \"Xvfb / software GL\"";
    printf "window_mapped_ms = %.3f\n", (mapped-start)/1000000;
    printf "idle_sample_seconds = %.3f\n", (idle_end-idle_start)/1000000000;
    printf "idle_cpu_percent_one_core = %.3f\n", 100*(last-first)/hz/((idle_end-idle_start)/1000000000);
    printf "idle_rss_kib = %d\n", rss;
  }' > "$output"
cat "$output"
