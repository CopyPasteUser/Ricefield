#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -r -- "$fixture"' EXIT

export HOME="$fixture/user with spaces"
export XDG_STATE_HOME="$fixture/state"
mkdir -p -- "$HOME/.local/bin"
install -m755 /bin/true "$HOME/.local/bin/ricefield"

for mode in default custom; do
  if [[ "$mode" == custom ]]; then
    export XDG_DATA_HOME="$fixture/custom data"
    data_dir="$XDG_DATA_HOME"
  else
    unset XDG_DATA_HOME || true
    data_dir="$HOME/.local/share"
  fi
  bash "$project_dir/scripts/install_desktop_entry.sh" "$project_dir"
  launcher="$data_dir/applications/ricefield.desktop"
  test -f "$launcher"
  test -f "$data_dir/icons/hicolor/scalable/apps/ricefield.svg"
  grep -Fqx -- "Exec=\"$HOME/.local/bin/ricefield\" %U" "$launcher"
  grep -Fqx -- 'Terminal=false' "$launcher"
  grep -Fqx -- 'Categories=System;FileManager;FileTools;' "$launcher"
  if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$launcher"
  fi
  bash "$project_dir/uninstall.sh"
  test ! -e "$launcher"
  test ! -e "$data_dir/icons/hicolor/scalable/apps/ricefield.svg"
  install -m755 /bin/true "$HOME/.local/bin/ricefield"
done

echo 'Desktop entry install, XDG registration and uninstall smoke passed'
