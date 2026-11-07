#!/usr/bin/env bash
set -euo pipefail

# Install from a verified release bundle without requiring Rust/Cargo.
bundle_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

bash "$bundle_dir/scripts/install_transaction.sh" "$bundle_dir/ricefield" "$bundle_dir"
