#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -r -- "$fixture"' EXIT
export HOME="$fixture/home with spaces"
export XDG_DATA_HOME="$fixture/XDG data"
export XDG_STATE_HOME="$fixture/XDG state"
mkdir -p -- "$HOME/.local/bin"

binary="$HOME/.local/bin/rice-explorer"
launcher="$XDG_DATA_HOME/applications/rice-explorer.desktop"
icon="$XDG_DATA_HOME/icons/hicolor/scalable/apps/rice-explorer.svg"

# Successful first install, then a regular upgrade replacing only the three
# owned paths. User config and unrelated application entries must survive.
mkdir -p -- "$XDG_DATA_HOME/applications" "$HOME/.config/rice-explorer"
printf 'keep me\n' > "$HOME/.config/rice-explorer/personal.toml"
printf '[Desktop Entry]\nType=Application\nName=Unrelated\nExec=/bin/true\n' \
  > "$XDG_DATA_HOME/applications/unrelated.desktop"
bash "$project_dir/scripts/install_transaction.sh" /bin/true "$project_dir"
test -x "$binary"
test -f "$launcher"
test -f "$icon"
install -m755 /bin/false "$fixture/new-executable"
bash "$project_dir/scripts/install_transaction.sh" "$fixture/new-executable" "$project_dir"
cmp -s -- "$fixture/new-executable" "$binary"

# A simulated desktop registration failure after partially touching its own
# launcher must restore *exactly* the existing binary, launcher and icon.
failure_project="$fixture/failing-project"
mkdir -p -- "$failure_project/scripts"
cp -- "$project_dir/scripts/user_install_lock.sh" "$failure_project/scripts/user_install_lock.sh"
printf '#!/usr/bin/env bash\nprintf broken > "%s"\nexit 23\n' "$launcher" \
  > "$failure_project/scripts/install_desktop_entry.sh"
cp -p -- "$binary" "$fixture/before-binary"
cp -p -- "$launcher" "$fixture/before-launcher"
cp -p -- "$icon" "$fixture/before-icon"
if bash "$project_dir/scripts/install_transaction.sh" /bin/true "$failure_project"; then
  echo 'Fault-injected installer unexpectedly succeeded' >&2
  exit 1
fi
cmp -s -- "$binary" "$fixture/before-binary"
cmp -s -- "$launcher" "$fixture/before-launcher"
cmp -s -- "$icon" "$fixture/before-icon"

# An upgrade must wait for the *same* lock rather than racing a competing
# updater's backups. The test holder owns a separate file descriptor/process.
lock_file="$XDG_STATE_HOME/rice-explorer/install.lock"
(
  exec 9>> "$lock_file"
  flock -x 9
  touch "$fixture/upgrade-lock-held"
  sleep 1
) &
holder=$!
for ((i = 0; i < 50; i++)); do
  [[ -f "$fixture/upgrade-lock-held" ]] && break
  sleep 0.02
done
test -f "$fixture/upgrade-lock-held"
bash "$project_dir/scripts/install_transaction.sh" /bin/true "$project_dir" > "$fixture/upgrade.log" 2>&1 &
upgrader=$!
sleep 0.2
kill -0 "$upgrader"
cmp -s -- "$binary" "$fixture/before-binary"
wait "$holder"
wait "$upgrader"
cmp -s -- /bin/true "$binary"

# The desktop icon must remain complete and readable while a replacement is
# still being copied. Simulate a slow copier that exposes a partial output:
# the staging path may be partial, but the published icon must stay intact.
mock_bin="$fixture/mock-bin"
mkdir -p -- "$mock_bin"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -euo pipefail' \
  'destination="${@: -1}"' \
  'printf partial-icon > "$destination"' \
  'touch "$RICEFIELD_TEST_INSTALL_PAUSED"' \
  'while [[ ! -f "$RICEFIELD_TEST_INSTALL_RESUME" ]]; do sleep 0.02; done' \
  '/usr/bin/install "$@"' > "$mock_bin/install"
chmod +x "$mock_bin/install"
cp -p -- "$icon" "$fixture/complete-icon"
export RICEFIELD_TEST_INSTALL_PAUSED="$fixture/icon-copy-paused"
export RICEFIELD_TEST_INSTALL_RESUME="$fixture/icon-copy-resume"
PATH="$mock_bin:$PATH" bash "$project_dir/scripts/install_desktop_entry.sh" "$project_dir" > "$fixture/icon-install.log" 2>&1 &
icon_installer=$!
for ((i = 0; i < 100; i++)); do
  [[ -e "$RICEFIELD_TEST_INSTALL_PAUSED" ]] && break
  sleep 0.02
done
if [[ ! -e "$RICEFIELD_TEST_INSTALL_PAUSED" ]]; then
  touch "$RICEFIELD_TEST_INSTALL_RESUME"
  wait "$icon_installer" || true
  echo 'Timed out waiting for simulated in-progress icon copy' >&2
  exit 1
fi
if ! cmp -s -- "$fixture/complete-icon" "$icon"; then
  touch "$RICEFIELD_TEST_INSTALL_RESUME"
  wait "$icon_installer" || true
  echo 'An in-progress icon copy exposed incomplete desktop assets' >&2
  exit 1
fi
touch "$RICEFIELD_TEST_INSTALL_RESUME"
wait "$icon_installer"
cmp -s -- "$icon" "$project_dir/assets/rice-explorer.svg"

# Installation must not follow a substituted state lock symlink or accept a
# relative XDG state root (which could target an unexpected current folder).
mv -- "$lock_file" "$fixture/preserved-install-lock"
ln -s -- "$fixture/preserved-install-lock" "$lock_file"
if bash "$project_dir/scripts/install_transaction.sh" /bin/false "$project_dir"; then
  echo 'Installer unexpectedly followed a symlinked lock' >&2
  exit 1
fi
test -L "$lock_file"
rm -- "$lock_file"
mv -- "$fixture/preserved-install-lock" "$lock_file"
if XDG_STATE_HOME=relative-state bash "$project_dir/scripts/install_transaction.sh" /bin/false "$project_dir"; then
  echo 'Installer unexpectedly accepted a relative state root' >&2
  exit 1
fi

# Symlinks may belong to another application; neither follow nor replace.
mv -- "$icon" "$fixture/preserved-icon"
ln -s -- "$fixture/preserved-icon" "$icon"
if bash "$project_dir/scripts/install_transaction.sh" /bin/true "$project_dir"; then
  echo 'Installer unexpectedly replaced an existing symlink' >&2
  exit 1
fi
test -L "$icon"
test -f "$HOME/.config/rice-explorer/personal.toml"
test -f "$XDG_DATA_HOME/applications/unrelated.desktop"

# An uninstall is serialized with install/update too.
(
  exec 9>> "$lock_file"
  flock -x 9
  touch "$fixture/uninstall-lock-held"
  sleep 1
) &
holder=$!
for ((i = 0; i < 50; i++)); do
  [[ -f "$fixture/uninstall-lock-held" ]] && break
  sleep 0.02
done
test -f "$fixture/uninstall-lock-held"
bash "$project_dir/uninstall.sh" > "$fixture/uninstall.log" 2>&1 &
uninstaller=$!
sleep 0.2
kill -0 "$uninstaller"
test -x "$binary"
wait "$holder"
wait "$uninstaller"
test ! -e "$binary"
test -f "$HOME/.config/rice-explorer/personal.toml"
echo 'Atomic installs, rollback, concurrent update/uninstall locks, symlink refusal and user-data preservation passed'
