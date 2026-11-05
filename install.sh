#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if ! command -v cargo >/dev/null 2>&1; then
  echo 'Rust/Cargo is required: https://rustup.rs' >&2
  exit 1
fi

cargo build --manifest-path "$project_dir/Cargo.toml" --locked --release
install -Dm755 "$project_dir/target/release/rice-explorer" "$HOME/.local/bin/rice-explorer"

# Desktop sessions do not always inherit ~/.local/bin in PATH. Register the
# executable with an absolute path, and respect XDG_DATA_HOME when configured.
bash "$project_dir/scripts/install_desktop_entry.sh" "$project_dir"
echo "Installed RiceExplorer and its application-menu launcher. Run: rice-explorer --theme win7"
