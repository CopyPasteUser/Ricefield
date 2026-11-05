#!/usr/bin/env bash
set -euo pipefail

# Must be run under both dbus-run-session and Xvfb, never the user's bus.
: "${RICE_EXPLORER_TEST_BUS:?Run with a private D-Bus session}"
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
mkdir -p "$fixture/files" "$fixture/config" "$fixture/state" "$fixture/cache" screenshots
printf 'Selected through FileManager1\n' > "$fixture/files/reveal.txt"
export XDG_CONFIG_HOME="$fixture/config" XDG_STATE_HOME="$fixture/state" XDG_CACHE_HOME="$fixture/cache"
export LIBGL_ALWAYS_SOFTWARE=1
export GSETTINGS_BACKEND=memory
./target/release/rice-explorer --file-manager-service "$fixture/files" > "$fixture/app.log" 2>&1 &
app_pid=$!
call_service() {
  timeout 5s gdbus call --session --dest org.freedesktop.FileManager1 \
    --object-path /org/freedesktop/FileManager1 \
    --method "org.freedesktop.FileManager1.$1" "$2" ''
}
window=''
for ((attempt = 0; attempt < 100; attempt++)); do
  kill -0 "$app_pid" 2>/dev/null || { cat "$fixture/app.log" >&2; exit 1; }
  window="$(xdotool search --onlyvisible --pid "$app_pid" --name '^RiceExplorer$' | head -n 1 || true)"
  if [[ -n "$window" ]] && call_service ShowItems "['file://$fixture/files/reveal.txt']" > /dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
[[ -n "$window" ]] || { cat "$fixture/app.log" >&2; exit 1; }
# Ensure the final call succeeded even if startup reached the last attempt.
call_service ShowItems "['file://$fixture/files/reveal.txt']" > /dev/null
sleep 1
import -window "$window" screenshots/file-manager-reveal.png
call_service ShowItemProperties "['file://$fixture/files/reveal.txt']" > /dev/null
sleep 1
import -window "$window" screenshots/file-manager-properties.png
difference="$(compare -metric AE screenshots/file-manager-reveal.png screenshots/file-manager-properties.png null: 2>&1 || true)"
if ! [[ "$difference" =~ ^[0-9]+$ ]] || (( difference < 500 )); then
  echo "Properties request did not visibly open a dialog ($difference pixels)" >&2
  exit 1
fi
if call_service ShowItems "['smb://user:secret@example.invalid/share']" > /dev/null 2>&1; then
  echo 'Unsafe URI was accepted' >&2
  exit 1
fi
kill -0 "$app_pid"
printf 'FileManager1 GUI requests and URI rejection passed\n'
