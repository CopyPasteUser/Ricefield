#!/usr/bin/env bash
set -euo pipefail

# Remove only files installed by install.sh. Keep user themes and settings.
user_home="${HOME:?HOME must be set}"
data_dir="${XDG_DATA_HOME:-$user_home/.local/share}"
case "$data_dir" in
  /*) ;;
  *) echo 'XDG_DATA_HOME must be an absolute path' >&2; exit 1 ;;
esac
rm -f -- \
  "$user_home/.local/bin/rice-explorer" \
  "$data_dir/applications/rice-explorer.desktop" \
  "$data_dir/icons/hicolor/scalable/apps/rice-explorer.svg"

echo 'Uninstalled RiceExplorer. User configuration and themes were preserved.'
