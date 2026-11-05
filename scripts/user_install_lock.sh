#!/usr/bin/env bash

# Shared advisory lock for install, upgrade and uninstall. Keep the lock file
# in user state: unlinking it after release would allow another process to
# lock a different inode while an older installer is still running.
rice_explorer_lock_installation() {
  local user_home="${HOME:?HOME must be set}"
  local state_root="${XDG_STATE_HOME:-$user_home/.local/state}"
  case "$state_root" in
    /*) ;;
    *) echo 'XDG_STATE_HOME must be an absolute path' >&2; return 1 ;;
  esac
  if ! command -v flock >/dev/null 2>&1; then
    echo 'RiceExplorer install/uninstall needs flock from util-linux' >&2
    return 1
  fi
  local lock_dir="$state_root/rice-explorer"
  local lock_file="$lock_dir/install.lock"
  if [[ -L "$lock_dir" ]]; then
    echo 'Refusing a symlinked RiceExplorer state directory' >&2
    return 1
  fi
  mkdir -p -m 700 -- "$lock_dir"
  if [[ -L "$lock_file" || ( -e "$lock_file" && ! -f "$lock_file" ) ]]; then
    echo 'Refusing a non-regular RiceExplorer installation lock' >&2
    return 1
  fi
  (umask 077; : >> "$lock_file")
  exec {RICE_EXPLORER_INSTALL_LOCK_FD}>> "$lock_file"
  if ! flock -x -w 30 "$RICE_EXPLORER_INSTALL_LOCK_FD"; then
    echo 'Another RiceExplorer install/uninstall still owns the lock' >&2
    return 1
  fi
}
