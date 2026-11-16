#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
usage() {
  cat <<'HELP'
Usage: bash install.sh [--default-file-manager]
       bash install.sh --default-file-manager-status
       bash install.sh --restore-default-file-manager

  (no args)                    Install/update RiceField without changing defaults
  --default-file-manager      Install/update and opt in as this user's default
  --default-file-manager-status  Inspect managed default registration (read-only)
  --restore-default-file-manager  Restore the previous default (no reinstall)

The default affects folder links, Xfce's file-manager helper, and new
FileManager1 D-Bus activations. It does not replace GTK applications' own
file dialogs, desktop icons, another user's preferences, or native DnD.
HELP
}

mode=install
if (( $# > 1 )); then
  usage >&2
  exit 2
fi
if (( $# == 1 )); then
  case "$1" in
    --default-file-manager) mode=install-default ;;
    --default-file-manager-status) mode=status ;;
    --restore-default-file-manager) mode=restore ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
fi

if [[ "$mode" == status || "$mode" == restore ]]; then
  python3 "$project_dir/scripts/default_file_manager.py" \
    "$( [[ "$mode" == status ]] && printf status || printf restore )"
  exit
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo 'Rust/Cargo is required: https://rustup.rs' >&2
  exit 1
fi

cargo build --manifest-path "$project_dir/Cargo.toml" --locked --release
bash "$project_dir/scripts/install_transaction.sh" \
  "$project_dir/target/release/ricefield" "$project_dir"

if [[ "$mode" == install-default ]]; then
  # Never silently sudo or overwrite other users' settings. The defaults
  # transaction records backups and will refuse conflicts on restoration.
  python3 "$project_dir/scripts/default_file_manager.py" enable
fi
