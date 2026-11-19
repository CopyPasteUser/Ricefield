#!/usr/bin/env bash
set -euo pipefail

# Native portal-picker screenshots, fully isolated from the runner's desktop.
# CI invokes this inside xvfb-run after building the release binary.
fixture="$(mktemp -d)"
picker_pid=''
cleanup() {
  if [[ -n "$picker_pid" ]]; then
    kill "$picker_pid" 2>/dev/null || true
    wait "$picker_pid" 2>/dev/null || true
  fi
  rm -rf -- "$fixture"
}
trap cleanup EXIT
mkdir -p "$fixture/Desktop" "$fixture/Documents" "$fixture/Downloads" \
  "$fixture/Pictures" "$fixture/files/Projects" screenshots
printf 'Notes for the picker demo\n' > "$fixture/files/Notes.txt"
printf 'Date,Amount\n2026-11-19,42\n' > "$fixture/files/Budget.csv"
printf '# Picker demo\n' > "$fixture/files/Readme.md"
export HOME="$fixture" XDG_CONFIG_HOME="$fixture/config"
export XDG_STATE_HOME="$fixture/state" XDG_CACHE_HOME="$fixture/cache"
export LIBGL_ALWAYS_SOFTWARE=1 GSETTINGS_BACKEND=memory

capture() {
  local mode="$1" title="$2" window='' output="screenshots/picker-$1.png"
  {
    printf 'mode = "%s"\n' "$mode"
    printf 'title = "%s"\n' "$title"
    printf 'folder = "file://%s/files"\n' "$fixture"
    if [[ "$mode" == save ]]; then
      printf 'current_name = "Monthly report.txt"\n'
    fi
    if [[ "$mode" == open ]]; then
      printf 'multiple = true\n'
      printf '[[filters]]\nname = "All files"\nrules = []\n'
    fi
  } > "$fixture/request.toml"

  ./target/release/ricefield --file-picker < "$fixture/request.toml" \
    > "$fixture/response.toml" 2> "$fixture/picker.log" &
  picker_pid=$!
  for ((attempt = 0; attempt < 80; attempt++)); do
    if ! kill -0 "$picker_pid" 2>/dev/null; then
      echo "Picker $mode crashed before its window appeared" >&2
      cat "$fixture/picker.log" >&2
      exit 1
    fi
    window="$(xdotool search --onlyvisible --pid "$picker_pid" --name 'RiceField.*File Picker' 2>/dev/null | head -n 1 || true)"
    [[ -z "$window" ]] || break
    sleep 0.25
  done
  if [[ -z "$window" ]]; then
    echo "Picker $mode window not found" >&2
    cat "$fixture/picker.log" >&2
    exit 1
  fi
  timeout 8s xdotool windowfocus --sync "$window"
  xdotool mousemove --window "$window" 800 575
  sleep 1
  import -window "$window" "$output"
  local dimensions
  dimensions="$(identify -format '%wx%h' "$output")"
  if [[ "$dimensions" != '840x620' ]]; then
    echo "Picker screenshot $mode has unexpected size $dimensions" >&2
    exit 1
  fi
  # Log a strictly synthetic fixture screenshot so a reviewer can inspect
  # native Xvfb pixels even when the artifact hand-off is not available.
  echo "PICKER_SCREENSHOT_$mode"_BEGIN
  base64 -w 0 "$output"
  echo
  echo "PICKER_SCREENSHOT_$mode"_END
  xdotool windowclose "$window"
  for ((attempt=0; attempt<40; attempt++)); do
    kill -0 "$picker_pid" 2>/dev/null || break
    sleep 0.1
  done
  if kill -0 "$picker_pid" 2>/dev/null; then
    echo "Picker $mode did not close" >&2
    exit 1
  fi
  wait "$picker_pid" || true
  picker_pid=''
}
capture open 'Open a document'
capture save 'Save a document'
difference="$(compare -metric AE screenshots/picker-open.png \
    screenshots/picker-save.png null: 2>&1 || true)"
if ! [[ "$difference" =~ ^[0-9]+$ ]] || (( difference < 500 )); then
  echo "Open and Save rendered indistinguishably ($difference pixels)" >&2
  exit 1
fi
echo 'Native FileChooser Open/Save screenshot smoke passed (Xvfb only).'
