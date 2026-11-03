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

export LIBGL_ALWAYS_SOFTWARE=1
for theme in win98 xp-luna win7 win10 frutiger-aero chromeos macos; do
  ./target/release/rice-explorer --theme "$theme" "$fixture_dir" > "$fixture_dir/app.log" 2>&1 &
  app_pid=$!
  sleep 5
  if ! kill -0 "$app_pid" 2>/dev/null; then
    echo "GUI exited unexpectedly in theme $theme" >&2
    sed -n '1,100p' "$fixture_dir/app.log" >&2
    exit 1
  fi
  # Capture the application, not merely an empty Xvfb root window.
  window_id="$(xdotool search --onlyvisible --name '^RiceExplorer$' | head -n 1 || true)"
  if [[ -z "$window_id" ]]; then
    echo "No visible RiceExplorer window for theme $theme" >&2
    sed -n '1,100p' "$fixture_dir/app.log" >&2
    exit 1
  fi
  import -window "$window_id" "screenshots/$theme.png"
  identify -format '%wx%h\n' "screenshots/$theme.png"
  kill "$app_pid" 2>/dev/null || true
  wait "$app_pid" 2>/dev/null || true
  app_pid=''
done
echo 'Native GUI smoke checks passed for all seven RiceBowl themes (window startup and screenshots only).'
