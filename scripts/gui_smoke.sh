#!/usr/bin/env bash
set -euo pipefail

# Run under xvfb-run. Smoke-test all bundled RiceBowl layouts without touching the host desktop.
fixture_dir="$(mktemp -d)"
display_dir="$fixture_dir/My Documents"
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

mkdir -p "$display_dir/Documents" "$display_dir/Pictures" "$display_dir/My Music" "$display_dir/My Pictures" "$display_dir/Projects" screenshots
printf 'RiceField native GUI smoke test\n' > "$display_dir/Readme.txt"
printf 'Personal notes for the GUI fixture\n' > "$display_dir/Notes.txt"
printf 'Month,Amount\nNovember,42\n' > "$display_dir/Budget.csv"
printf '# Markdown preview test\n' > "$display_dir/Documents/Example.md"
cp assets/rice-explorer.png "$display_dir/Pictures/icon.png"
cp themes/win7.toml "$fixture_dir/custom-theme.toml"
# Exercise a *real structural* shell reordering rather than merely recoloring
# the built-in Aero surface. Keep the custom file independent of bundled rice.
sed -i 's/^top = \["tabs", "navigation", "command-bar"\]$/top = ["navigation"]\nbottom = ["tabs", "command-bar"]/' "$fixture_dir/custom-theme.toml"
grep -Fqx 'bottom = ["tabs", "command-bar"]' "$fixture_dir/custom-theme.toml"
printf '\n[shell.sidebar]\nposition = "right"\nwidth = 270\nsections = ["volumes", "favorites"]\n' >> "$fixture_dir/custom-theme.toml"

export LIBGL_ALWAYS_SOFTWARE=1
export GSETTINGS_BACKEND=memory
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
    ./target/release/rice-explorer --theme-file "$fixture_dir/custom-theme.toml" "$display_dir" > "$fixture_dir/app.log" 2>&1 &
  else
    ./target/release/rice-explorer --theme "$theme" "$display_dir" > "$fixture_dir/app.log" 2>&1 &
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
    window_id="$(xdotool search --onlyvisible --name '^RiceField$' | head -n 1 || true)"
    if [[ -n "$window_id" ]]; then
      break
    fi
    sleep 1
  done
  if [[ -z "$window_id" ]]; then
    echo "No visible RiceField window for theme $theme after 35 seconds" >&2
    report_startup_failure "$theme"
    exit 1
  fi
  # xvfb-run has no window manager, so the X input focus can remain on the
  # root/None window. Explicitly focus the app and use XTEST key injection;
  # xdotool --window sends synthetic events that egui/winit may not accept.
  if ! timeout 8s xdotool windowfocus --sync "$window_id"; then
    echo "Cannot focus $theme within eight seconds (Xvfb may lack a responsive window manager)" >&2
    report_startup_failure "$theme"
    exit 1
  fi
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
  # Windows 10 has distinct interactive ribbon pages, not merely a painted
  # Home toolbar. Verify that both other pages visibly change the running UI.
  if [[ "$theme" == win10 ]]; then
    xdotool mousemove --window "$window_id" 221 21 click 1
    sleep 1
    xdotool mousemove --window "$window_id" 1050 650
    import -window "$window_id" screenshots/win10-view.png
    require_visible_change screenshots/win10.png screenshots/win10-view.png 'Windows 10 View ribbon' 1500
    xdotool mousemove --window "$window_id" 150 21 click 1
    sleep 1
    xdotool mousemove --window "$window_id" 1050 650
    import -window "$window_id" screenshots/win10-share.png
    require_visible_change screenshots/win10.png screenshots/win10-share.png 'Windows 10 Share ribbon' 1500
    xdotool mousemove --window "$window_id" 81 21 click 1
    sleep 1
  fi
  # Aqua's glass shelf must reveal actual advanced Explorer tools when clicked,
  # not merely paint a decorative dock. This is a stable 1120x720 smoke hit
  # target; human keyboard/focus acceptance remains a separate requirement.
  if [[ "$theme" == frutiger-aero ]]; then
    xdotool mousemove --window "$window_id" 735 645 click 1
    sleep 0.6
    xdotool mousemove --window "$window_id" 1050 650
    import -window "$window_id" screenshots/frutiger-aero-tools.png
    require_visible_change screenshots/frutiger-aero.png screenshots/frutiger-aero-tools.png \
      'Frutiger Aero dock Tools action' 1500
    xdotool mousemove --window "$window_id" 735 645 click 1
    sleep 0.25
  fi
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
    # Let egui process each command as a separate frame. Sending the whole
    # sequence in one XTEST burst can coalesce events and leave focus stale.
    sleep 0.15
  done
  # All skins must be able to open and dismiss a native settings/action dialog
  # without trapping keyboard input or creating a test directory.
  # First move keyboard focus off any editable tab/address widget. This also
  # checks that Ctrl+N works after clicking the file area, not merely at launch.
  xdotool mousemove --window "$window_id" 620 420 click 1
  sleep 0.2
  import -window "$window_id" "screenshots/$theme-before-dialog.png"
  xdotool key ctrl+n
  sleep 0.45
  import -window "$window_id" "screenshots/$theme-dialog.png"
  require_visible_change "screenshots/$theme-before-dialog.png" "screenshots/$theme-dialog.png" "$theme dialog opening" 500
  xdotool key Escape
  sleep 0.45
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
      extra_window_id="$(xdotool search --onlyvisible --name '^RiceField$' 2>/dev/null | grep -vx "$window_id" | head -n 1 || true)"
      if [[ -n "$extra_window_id" ]]; then
        break
      fi
      sleep 0.25
    done
    if [[ -z "$extra_window_id" ]]; then
      echo 'Ctrl+Shift+N failed to open a second native RiceField window' >&2
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
# These pixel differences are a cheap regression alarm, not visual approval:
# a renderer accidentally collapsing all rice profiles into one appearance
# should fail CI even if each process still starts successfully.
require_visible_change "screenshots/win98.png" "screenshots/win7.png" "Win98 vs Win7 rice identity" 12000
require_visible_change "screenshots/win7.png" "screenshots/macos.png" "Win7 vs macOS rice identity" 12000
require_visible_change "screenshots/win98.png" "screenshots/macos.png" "Win98 vs macOS rice identity" 12000
require_visible_change "screenshots/win7.png" "screenshots/custom-sidebar.png" "Custom shell recomposition" 12000
echo 'Native GUI smoke checks passed for seven themes and one structurally recomposed custom rice (including pairwise visual difference checks; not physical acceptance).'
