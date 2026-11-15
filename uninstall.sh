#!/usr/bin/env bash
set -euo pipefail

# Remove only files installed by install.sh. Keep user themes and settings.
user_home="${HOME:?HOME must be set}"
data_dir="${XDG_DATA_HOME:-$user_home/.local/share}"
case "$data_dir" in
  /*) ;;
  *) echo 'XDG_DATA_HOME must be an absolute path' >&2; exit 1 ;;
esac
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
source "$script_dir/scripts/user_install_lock.sh"
ricefield_lock_installation
# A default association to a removed binary would leave external folder
# launchers broken. Require an explicit, verified restore first.
defaults_receipt="${XDG_STATE_HOME:-$user_home/.local/state}/ricefield/default-file-manager/receipt.json"
if [[ -e "$defaults_receipt" || -L "$defaults_receipt" ]]; then
  echo 'RiceField is configured as the user default; run python3 scripts/default_file_manager.py restore before uninstalling.' >&2
  exit 1
fi
rm -f -- \
  "$user_home/.local/bin/ricefield" \
  "$data_dir/applications/ricefield.desktop" \
  "$data_dir/icons/hicolor/scalable/apps/ricefield.svg" \
  "$data_dir/icons/hicolor/256x256/apps/ricefield.png"

echo 'Uninstalled RiceField. User configuration and themes were preserved.'
