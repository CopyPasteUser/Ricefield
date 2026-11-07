#!/usr/bin/env bash
set -euo pipefail

project_dir="${1:?Expected RiceField source directory}"
user_home="${HOME:?HOME must be set}"
data_dir="${XDG_DATA_HOME:-$user_home/.local/share}"

case "$data_dir" in
  /*) ;;
  *) echo 'XDG_DATA_HOME must be an absolute path' >&2; exit 1 ;;
esac

binary="$user_home/.local/bin/ricefield"
if [[ ! -x "$binary" ]]; then
  echo "RiceField executable is missing: $binary" >&2
  exit 1
fi

# Desktop Entry Exec fields require quoting and double escaping for special
# characters inside quoted executable paths. Percent signs are field codes.
escaped="$binary"
escaped="${escaped//\\/\\\\}"
escaped="${escaped//\"/\\\"}"
escaped="${escaped//\$/\\\$}"
escaped="${escaped//\`/\\\`}"
escaped="${escaped//%/%%}"

apps_dir="$data_dir/applications"
mkdir -p -- "$apps_dir"
temporary="$(mktemp "$apps_dir/.ricefield.XXXXXX.desktop")"
icon_temporary=''
cleanup() {
  [[ -z "$temporary" ]] || rm -f -- "$temporary"
  [[ -z "$icon_temporary" ]] || rm -f -- "$icon_temporary"
}
trap cleanup EXIT
while IFS= read -r line || [[ -n "$line" ]]; do
  case "$line" in
    Exec=*) printf 'Exec="%s" %%U\n' "$escaped" ;;
    *) printf '%s\n' "$line" ;;
  esac
done < "$project_dir/assets/ricefield.desktop" > "$temporary"
chmod 0644 "$temporary"
if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$temporary"
fi
mv -fT -- "$temporary" "$apps_dir/ricefield.desktop"
temporary=''

icon_dir="$data_dir/icons/hicolor/scalable/apps"
mkdir -p -- "$icon_dir"
icon_temporary="$(mktemp "$icon_dir/.ricefield.XXXXXX.svg")"
# Never write into the published icon path. A killed process during copying
# must leave the previous complete icon visible until the final rename.
install -m644 -- "$project_dir/assets/ricefield.svg" "$icon_temporary"
mv -fT -- "$icon_temporary" "$icon_dir/ricefield.svg"
icon_temporary=''

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$apps_dir" || true
fi
if command -v xdg-desktop-menu >/dev/null 2>&1; then
  xdg-desktop-menu forceupdate --mode user >/dev/null 2>&1 || true
fi
