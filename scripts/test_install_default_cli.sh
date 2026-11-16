#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/source/assets" "$fixture/source/scripts" "$fixture/bin" \
  "$fixture/home/.config/xfce4" "$fixture/home/.local/share" "$fixture/state"
cp "$project_dir/install.sh" "$fixture/source/"
cp "$project_dir/assets/ricefield.desktop" "$project_dir/assets/ricefield.svg" \
  "$project_dir/assets/ricefield.png" "$fixture/source/assets/"
cp "$project_dir/scripts/install_transaction.sh" \
  "$project_dir/scripts/install_desktop_entry.sh" \
  "$project_dir/scripts/cleanup_legacy.sh" \
  "$project_dir/scripts/user_install_lock.sh" \
  "$project_dir/scripts/default_file_manager.py" "$fixture/source/scripts/"

cat > "$fixture/bin/cargo" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
while (( $# )); do
  if [[ "$1" == --manifest-path ]]; then
    shift
    project_dir="$(dirname -- "$1")"
    mkdir -p "$project_dir/target/release"
    cp /bin/true "$project_dir/target/release/ricefield"
    chmod +x "$project_dir/target/release/ricefield"
    exit
  fi
  shift
done
exit 2
MOCK
chmod 755 "$fixture/bin/cargo"
export HOME="$fixture/home"
export XDG_CONFIG_HOME="$fixture/home/.config"
export XDG_DATA_HOME="$fixture/home/.local/share"
export XDG_STATE_HOME="$fixture/state"
export PATH="$fixture/bin:$PATH"

cat > "$XDG_CONFIG_HOME/mimeapps.list" <<'INI'
[Default Applications]
inode/directory=thunar.desktop;
text/plain=mousepad.desktop;
INI
original="$(cat "$XDG_CONFIG_HOME/mimeapps.list")"

# A status query and invalid arguments must not install anything.
bash "$fixture/source/install.sh" --default-file-manager-status > "$fixture/status"
grep -q 'Managed receipt: no' "$fixture/status"
if bash "$fixture/source/install.sh" --invalid > /dev/null 2>&1; then
  echo 'Unknown install option unexpectedly accepted' >&2
  exit 1
fi
test ! -e "$HOME/.local/bin/ricefield"

bash "$fixture/source/install.sh" --default-file-manager
bash "$fixture/source/install.sh" --default-file-manager

test -x "$HOME/.local/bin/ricefield"
grep -q '^inode/directory=ricefield.desktop;' "$XDG_CONFIG_HOME/mimeapps.list"
grep -q '^text/plain=mousepad.desktop;' "$XDG_CONFIG_HOME/mimeapps.list"
grep -q '^FileManager=ricefield' "$XDG_CONFIG_HOME/xfce4/helpers.rc"
grep -q '^Name=org.freedesktop.FileManager1' \
  "$XDG_DATA_HOME/dbus-1/services/org.freedesktop.FileManager1.service"
bash "$fixture/source/install.sh" --default-file-manager-status > "$fixture/status"
grep -q 'Managed receipt: yes' "$fixture/status"

# Restoration changes only the registered defaults; the executable stays.
bash "$fixture/source/install.sh" --restore-default-file-manager
[[ "$(cat "$XDG_CONFIG_HOME/mimeapps.list")" == "$original" ]]
test -x "$HOME/.local/bin/ricefield"
test ! -e "$XDG_DATA_HOME/dbus-1/services/org.freedesktop.FileManager1.service"

# Ordinary upgrades do not reactivate the default.
bash "$fixture/source/install.sh"
[[ "$(cat "$XDG_CONFIG_HOME/mimeapps.list")" == "$original" ]]

echo 'RiceField installer default-manager opt-in, status, restore and upgrade passed'
