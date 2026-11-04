#!/usr/bin/env bash
set -euo pipefail

# Remove only files installed by install.sh. Keep user themes and settings.
user_home="${HOME:?HOME must be set}"
rm -f -- \
  "$user_home/.local/bin/rice-explorer" \
  "$user_home/.local/share/applications/rice-explorer.desktop" \
  "$user_home/.local/share/icons/hicolor/scalable/apps/rice-explorer.svg"

echo 'Uninstalled RiceExplorer. User configuration and themes were preserved.'
