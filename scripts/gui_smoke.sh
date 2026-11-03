#!/usr/bin/env bash
set -euo pipefail

# Run under xvfb-run. Exercise three distinct RiceBowl layouts without touching the host desktop.
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
for theme in win7 xp-luna chromeos; do
  ./target/release/rice-explorer --theme "$theme" "$fixture_dir" > "$fixture_dir/app.log" 2>&1 &
  app_pid=$!
  sleep 5
  if ! kill -0 "$app_pid" 2>/dev/null; then
    echo "GUI exited unexpectedly in theme $theme" >&2
    sed -n '1,100p' "$fixture_dir/app.log" >&2
    exit 1
  fi
  import -window root "screenshots/$theme.png"
  kill "$app_pid" 2>/dev/null || true
  wait "$app_pid" 2>/dev/null || true
  app_pid=''
done
echo 'Native GUI smoke checks passed for Windows 7, XP and ChromeOS profiles.'
