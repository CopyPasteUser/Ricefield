#!/usr/bin/env bash
set -euo pipefail

# The pre-rename RiceExplorer installer owned these exact user-local paths.
# Keep old configuration and themes: they can contain irreplaceable user data.
user_home="${HOME:?HOME must be set}"
data_dir="${XDG_DATA_HOME:-$user_home/.local/share}"
case "$data_dir" in
  /*) ;;
  *) echo 'XDG_DATA_HOME must be an absolute path' >&2; exit 1 ;;
esac

legacy_paths=(
  "$user_home/.local/bin/rice-explorer"
  "$data_dir/applications/rice-explorer.desktop"
  "$data_dir/icons/hicolor/scalable/apps/rice-explorer.svg"
)
cargo_root="${CARGO_HOME:-$user_home/.cargo}"
if [[ "$cargo_root" == /* ]]; then
  legacy_paths+=("$cargo_root/bin/rice-explorer")
fi

for path in "${legacy_paths[@]}"; do
  if [[ -f "$path" || -L "$path" ]]; then
    rm -f -- "$path"
  fi
done

# Older interrupted installs could leave hidden .rice-explorer.*.desktop files.
apps_dir="$data_dir/applications"
if [[ -d "$apps_dir" ]]; then
  for path in "$apps_dir"/.rice-explorer.*.desktop; do
    if [[ -f "$path" && ! -L "$path" ]]; then
      rm -f -- "$path"
    fi
  done
fi
