#!/usr/bin/env bash
set -euo pipefail

# Run under xvfb-run. Smoke-test all bundled RiceBowl layouts without touching the host desktop.
fixture_dir="$(mktemp -d)"
app_pid=''
cleanup() {
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  if [[ -d "$fixture_dir" ]]; then
    rm -r -- "$fixture_dir"
  fi
}
trap cleanup EXIT

mkdir -p "$fixture_dir/Documents" "$fixture_dir/Pictures" screenshots
printf 'RiceExplorer native GUI smoke test\n' > "$fixture_dir/Readme.txt"
printf '# Markdown preview test\n' > "$fixture_dir/Documents/Example.md"
cp assets/rice-explorer.png "$fixture_dir/Pictures/icon.png"
cp themes/win7.toml "$fixture_dir/custom-theme.toml"
printf '\n[shell.sidebar]\nposition = "right"\nwidth = 270\nsections = ["volumes", "favorites"]\n' >> "$fixture_dir/custom-theme.toml"

export LIBGL_ALWAYS_SOFTWARE=1
# Never read or modify a developer's real config/state/cache, and never let
# intentionally unfinished journals from cargo tests contaminate screenshots.
export XDG_STATE_HOME="$fixture_dir/.state"
export XDG_CONFIG_HOME="$fixture_dir/.config"
export XDG_CACHE_HOME="$fixture_dir/.cache"

report_startup_failure() {
  local theme="$1"
  echo "GUI startup failed for theme $theme (DISPLAY=${DISPLAY:-unset})" >&2
  ps -o pid,etime,stat,cmd -p "$app_pid" >&2 || true
  echo 'Visible X11 window titles:' >&2
  while IFS= read -r id; do
    xdotool getwindowname "$id" >&2 || true
  done < <(xdotool search --onlyvisible --name '.*' 2>/dev/null | head -n 12 || true)
  echo 'Application stderr/stdout:' >&2
  sed -n '1,100p' "$fixture_dir/app.log" >&2
}

for theme in win98 xp-luna win7 win10 frutiger-aero chromeos macos custom-sidebar; do
  if [[ "$theme" == custom-sidebar ]]; then
    ./target/release/rice-explorer --theme-file "$fixture_dir/custom-theme.toml" "$fixture_dir" > "$fixture_dir/app.log" 2>&1 &
  else
    ./target/release/rice-explorer --theme "$theme" "$fixture_dir" > "$fixture_dir/app.log" 2>&1 &
  fi
  app_pid=$!
  # Xvfb/Mesa can take longer to initialize on a cold CI runner. Poll for the
  # mapped application window instead of assuming startup always takes 5s.
  window_id=''
  for ((attempt = 0; attempt < 35; attempt++)); do
    if ! kill -0 "$app_pid" 2>/dev/null; then
      echo "GUI exited unexpectedly in theme $theme" >&2
      report_startup_failure "$theme"
      exit 1
    fi
    window_id="$(xdotool search --onlyvisible --name '^RiceExplorer$' | head -n 1 || true)"
    if [[ -n "$window_id" ]]; then
      break
    fi
    sleep 1
  done
  if [[ -z "$window_id" ]]; then
    echo "No visible RiceExplorer window for theme $theme after 35 seconds" >&2
    report_startup_failure "$theme"
    exit 1
  fi
  # Avoid capturing an incidental hover tooltip over the file list.
  xdotool mousemove --window "$window_id" 1050 650
  sleep 1
  import -window "$window_id" "screenshots/$theme.png"
  identify -format '%wx%h\n' "screenshots/$theme.png"
  # Exercise the new two-pane command on every shell; a screenshot is a smoke
  # artifact, not proof of correct hit targets or system drag-and-drop.
  xdotool key --window "$window_id" F3
  sleep 1
  if ! kill -0 "$app_pid" 2>/dev/null; then
    echo "Split view crashed in theme $theme" >&2
    report_startup_failure "$theme"
    exit 1
  fi
  import -window "$window_id" "screenshots/$theme-split.png"
  identify -format '%wx%h\n' "screenshots/$theme-split.png"
  xdotool key --window "$window_id" F3
  kill "$app_pid" 2>/dev/null || true
  wait "$app_pid" 2>/dev/null || true
  app_pid=''
done
echo 'Native GUI smoke checks passed for seven themes and one custom sidebar (startup, F3 toggle and screenshots; not physical acceptance).'
