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
bash "$bundle_dir/scripts/install_desktop_entry.sh" "$bundle_dir"

echo 'Installed RiceExplorer and its application-menu launcher from the release bundle.'
