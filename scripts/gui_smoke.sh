#!/usr/bin/env bash
set -euo pipefail

# Run under xvfb-run. Smoke-test all bundled RiceBowl layouts without touching the host desktop.
fixture_dir="$(mktemp -d)"
app_pid=''
extra_window_id=''
cleanup() {
  if [[ -n "$extra_window_id" ]]; then
    xdotool windowclose "$extra_window_id" 2>/dev/null || true
  fi
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

require_visible_change() {
  local before="$1" after="$2" description="$3" minimum="$4" difference
  # ImageMagick exits 1 when images differ; the numeric AE metric is printed
  # on stderr. Never hide missing/corrupt screenshots behind that exit code.
  difference="$(compare -metric AE "$before" "$after" null: 2>&1 || true)"
  if ! [[ "$difference" =~ ^[0-9]+$ ]] || (( difference < minimum )); then
    echo "$description did not visibly change ($difference pixels; expected >= $minimum)" >&2
    exit 1
  fi
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
  # xvfb-run has no window manager, so the X input focus can remain on the
  # root/None window. Explicitly focus the app and use XTEST key injection;
  # xdotool --window sends synthetic events that egui/winit may not accept.
  xdotool windowfocus --sync "$window_id"
  focused_window_id="$(xdotool getwindowfocus)"
  if [[ "$focused_window_id" != "$window_id" ]]; then
    echo "Cannot focus $theme: expected $window_id, got $focused_window_id" >&2
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
  xdotool key F3
  sleep 1
  if ! kill -0 "$app_pid" 2>/dev/null; then
    echo "Split view crashed in theme $theme" >&2
    report_startup_failure "$theme"
    exit 1
  fi
  import -window "$window_id" "screenshots/$theme-split.png"
  identify -format '%wx%h\n' "screenshots/$theme-split.png"
  require_visible_change "screenshots/$theme.png" "screenshots/$theme-split.png" "$theme split view" 1000
  xdotool key F3
  # Exercise tab creation, cycling, reordering, closure and recovery in every
  # rice. These are crash/regression smokes, not accessibility certification.
  for combo in ctrl+t ctrl+Page_Down ctrl+shift+Page_Up ctrl+w ctrl+shift+t; do
    xdotool key "$combo"
  done
  # All skins must be able to open and dismiss a native settings/action dialog
  # without trapping keyboard input or creating a test directory.
  xdotool key ctrl+n
  sleep 0.3
  import -window "$window_id" "screenshots/$theme-dialog.png"
  xdotool key Escape
  sleep 0.3
  import -window "$window_id" "screenshots/$theme-dialog-closed.png"
  require_visible_change "screenshots/$theme-dialog.png" "screenshots/$theme-dialog-closed.png" "$theme dialog dismissal" 500
  # Check compact split-capable layout in the project's supported minimum
  # window range. Screenshots provide review evidence, not hit-target proof.
  xdotool windowsize "$window_id" 700 470
  sleep 0.4
  if ! kill -0 "$app_pid" 2>/dev/null; then
    echo "Compact layout crashed in theme $theme" >&2
    report_startup_failure "$theme"
    exit 1
  fi
  import -window "$window_id" "screenshots/$theme-compact.png"
  identify -format '%wx%h\n' "screenshots/$theme-compact.png"
  xdotool windowsize "$window_id" 380 650
  sleep 0.4
  if ! kill -0 "$app_pid" 2>/dev/null; then
    echo "Narrow window crashed in theme $theme" >&2
    report_startup_failure "$theme"
    exit 1
  fi
  import -window "$window_id" "screenshots/$theme-narrow.png"
  identify -format '%wx%h\n' "screenshots/$theme-narrow.png"
  xdotool windowsize "$window_id" 1120 720
  # The Windows 7 profile also proves a second native process/window can
  # launch without replacing the existing window. Close it before the next
  # profile; no desktop session or personal settings are touched.
  if [[ "$theme" == win7 ]]; then
    xdotool key ctrl+shift+n
    for ((attempt = 0; attempt < 40; attempt++)); do
      extra_window_id="$(xdotool search --onlyvisible --name '^RiceExplorer$' 2>/dev/null | grep -vx "$window_id" | head -n 1 || true)"
      if [[ -n "$extra_window_id" ]]; then
        break
      fi
      sleep 0.25
    done
    if [[ -z "$extra_window_id" ]]; then
      echo 'Ctrl+Shift+N failed to open a second native RiceExplorer window' >&2
      report_startup_failure "$theme"
      exit 1
    fi
    xdotool windowclose "$extra_window_id" 2>/dev/null || true
    extra_window_id=''
  fi
  kill "$app_pid" 2>/dev/null || true
  wait "$app_pid" 2>/dev/null || true
  app_pid=''
done
echo 'Native GUI smoke checks passed for seven themes and one custom sidebar (startup, F3, tab/keyboard actions, dialog dismissal, compact layouts, and secondary window; not physical acceptance).'
