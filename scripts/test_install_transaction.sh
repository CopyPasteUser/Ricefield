#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
fixture="$(mktemp -d)"
trap 'rm -r -- "$fixture"' EXIT
export HOME="$fixture/home with spaces"
export XDG_DATA_HOME="$fixture/XDG data"
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
echo 'Atomic binary install, upgrade, rollback, symlink refusal and user-data preservation passed'
