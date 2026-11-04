#!/usr/bin/env bash
set -euo pipefail

# Install from a verified release bundle without requiring Rust/Cargo.
bundle_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
user_home="${HOME:?HOME must be set}"

if [[ ! -x "$bundle_dir/rice-explorer" ]]; then
  echo 'RiceExplorer executable is missing from this release bundle' >&2
  exit 1
fi

install -Dm755 -- "$bundle_dir/rice-explorer" "$user_home/.local/bin/rice-explorer"
install -Dm644 -- "$bundle_dir/assets/rice-explorer.desktop" \
  "$user_home/.local/share/applications/rice-explorer.desktop"
install -Dm644 -- "$bundle_dir/assets/rice-explorer.svg" \
  "$user_home/.local/share/icons/hicolor/scalable/apps/rice-explorer.svg"

echo 'Installed RiceExplorer from the release bundle. Run: rice-explorer --theme win7'
