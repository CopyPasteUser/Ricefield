#!/usr/bin/env bash
set -euo pipefail

# Install the binary by rename, then register the desktop integration. An
# ordinary registration failure restores all three previously installed files.
# This does not claim power-loss atomicity across multiple directories.
source_binary="${1:?Expected an executable RiceExplorer binary}"
project_dir="${2:?Expected a RiceExplorer source or release bundle directory}"
user_home="${HOME:?HOME must be set}"
data_dir="${XDG_DATA_HOME:-$user_home/.local/share}"

case "$data_dir" in
  /*) ;;
  *) echo 'XDG_DATA_HOME must be an absolute path' >&2; exit 1 ;;
esac
if [[ ! -f "$source_binary" || ! -x "$source_binary" ]]; then
  echo 'RiceExplorer installation source is missing or not executable' >&2
  exit 1
fi

# Acquire before inspecting any destination: a failed upgrade must never
# restore stale backups on top of a newer concurrently installed version.
source "$project_dir/scripts/user_install_lock.sh"
rice_explorer_lock_installation

targets=(
  "$user_home/.local/bin/rice-explorer"
  "$data_dir/applications/rice-explorer.desktop"
  "$data_dir/icons/hicolor/scalable/apps/rice-explorer.svg"
)
for target in "${targets[@]}"; do
  if [[ -L "$target" || ( -e "$target" && ! -f "$target" ) ]]; then
    echo "Refusing to replace an existing non-regular installation path: $target" >&2
    exit 1
  fi
done

backup_dir="$(mktemp -d)"
previous=(0 0 0)
published=0
binary_tmp=''
on_exit() {
  local result=$? restored=1 i rollback_tmp
  trap - EXIT
  if [[ -n "$binary_tmp" ]]; then
    rm -f -- "$binary_tmp"
  fi
  if (( result != 0 && published == 1 )); then
    for i in "${!targets[@]}"; do
      if [[ "${previous[$i]}" == 1 ]]; then
        # Restore by rename too, so a running former binary stays intact.
        rollback_tmp="$(mktemp "$(dirname -- "${targets[$i]}")/.rice-explorer-rollback.XXXXXX")" || { restored=0; continue; }
        if ! cp -p -- "$backup_dir/$i" "$rollback_tmp" || ! mv -f -- "$rollback_tmp" "${targets[$i]}"; then
          restored=0
          rm -f -- "$rollback_tmp"
        fi
      elif ! rm -f -- "${targets[$i]}"; then
        restored=0
      fi
    done
    if (( restored == 1 )); then
      echo 'RiceExplorer install failed; previous binary and launcher restored.' >&2
    else
      echo 'RiceExplorer install failed and rollback was incomplete; inspect the three installation paths.' >&2
    fi
  fi
  rm -r -- "$backup_dir"
  exit "$result"
}
trap on_exit EXIT

for i in "${!targets[@]}"; do
  if [[ -e "${targets[$i]}" ]]; then
    cp -p -- "${targets[$i]}" "$backup_dir/$i"
    previous[$i]=1
  fi
done

mkdir -p -- "$(dirname -- "${targets[0]}")"
binary_tmp="$(mktemp "$(dirname -- "${targets[0]}")/.rice-explorer.XXXXXX")"
install -m755 -- "$source_binary" "$binary_tmp"
mv -f -- "$binary_tmp" "${targets[0]}"
binary_tmp=''
published=1
# The parent keeps the advisory lock. Do not leak its descriptor into desktop
# cache helpers that might spawn longer-lived background processes.
(
  exec {RICE_EXPLORER_INSTALL_LOCK_FD}>&-
  bash "$project_dir/scripts/install_desktop_entry.sh" "$project_dir"
)
echo 'Installed RiceExplorer and its application-menu launcher.'
