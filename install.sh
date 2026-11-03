#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if ! command -v cargo >/dev/null 2>&1; then
  echo 'Rust/Cargo is required: https://rustup.rs' >&2
  exit 1
fi

cargo build --manifest-path "$project_dir/Cargo.toml" --release
install -Dm755 "$project_dir/target/release/rice-explorer" "$HOME/.local/bin/rice-explorer"
install -Dm644 "$project_dir/assets/rice-explorer.desktop" "$HOME/.local/share/applications/rice-explorer.desktop"
install -Dm644 "$project_dir/assets/rice-explorer.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/rice-explorer.svg"
echo "Installed RiceExplorer. Run: rice-explorer --theme win7"
