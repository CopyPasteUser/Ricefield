#!/usr/bin/env bash
set -euo pipefail

# Exercise real editable controls; a menu that closes when its header or input
# is clicked must fail this test instead of merely producing a screenshot.
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
mkdir -p "$fixture/config/rice-explorer" "$fixture/state" "$fixture/cache" "$fixture/files" screenshots
printf 'one' > "$fixture/files/report-42.pdf"
printf 'two' > "$fixture/files/other.txt"
printf "filter_name_regex = '['\n" > "$fixture/config/rice-explorer/config.toml"
export XDG_CONFIG_HOME="$fixture/config" XDG_STATE_HOME="$fixture/state" XDG_CACHE_HOME="$fixture/cache"
export LIBGL_ALWAYS_SOFTWARE=1 GSETTINGS_BACKEND=memory
./target/release/rice-explorer --theme win7 "$fixture/files" > "$fixture/app.log" 2>&1 &
app_pid=$!
window=''
for ((attempt = 0; attempt < 100; attempt++)); do
  kill -0 "$app_pid" 2>/dev/null || { cat "$fixture/app.log" >&2; exit 1; }
  window="$(xdotool search --onlyvisible --pid "$app_pid" --name '^RiceField$' | head -n 1 || true)"
  [[ -z "$window" ]] || break
  sleep 0.1
done
[[ -n "$window" ]] || { cat "$fixture/app.log" >&2; exit 1; }
timeout 5s xdotool windowfocus --sync "$window"
sleep 0.5
click_at() {
  xdotool mousemove --window "$window" "$1" "$2"
  sleep 0.15
  xdotool click 1
  sleep 0.3
}
# The Aero shell intentionally keeps advanced tools hidden until requested.
# Exercise that user-facing disclosure before opening the editable Filters
# menu. The old positions assumed the now-removed generic Explorer toolbar.
click_at 680 55  # More tools (Aero command strip)
click_at 660 155 # Filters in the revealed tools row
click_at 750 305 # Custom pattern and ranges
import -window "$window" screenshots/filters-invalid-regex.png
click_at 750 418 # Filename regex
xdotool key ctrl+a
xdotool type --delay 20 '42'
for ((attempt = 0; attempt < 50; attempt++)); do
  if grep -Fqx 'filter_name_regex = "42"' "$fixture/config/rice-explorer/config.toml"; then
    break
  fi
  sleep 0.1
done
if ! grep -Fqx 'filter_name_regex = "42"' "$fixture/config/rice-explorer/config.toml"; then
  import -window "$window" screenshots/filters-edit-failed.png
  echo 'Custom filter input did not accept and persist editing' >&2
  exit 1
fi
xdotool key Escape
sleep 0.3
import -window "$window" screenshots/filters-valid-regex.png
printf 'Editable filter menu, regex correction and persistence passed\n'
