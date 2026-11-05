#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if ! command -v cargo >/dev/null 2>&1; then
  echo 'Rust/Cargo is required: https://rustup.rs' >&2
  exit 1
fi

cargo build --manifest-path "$project_dir/Cargo.toml" --locked --release
bash "$project_dir/scripts/install_transaction.sh" \
  "$project_dir/target/release/rice-explorer" "$project_dir"
