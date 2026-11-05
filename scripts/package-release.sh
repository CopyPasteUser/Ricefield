#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
binary_path="${1:-$repo_dir/target/release/rice-explorer}"
output_dir="${2:-$repo_dir/dist}"
release_arch="${RICE_EXPLORER_ARCH:-$(uname -m)}"
release_version="$(sed -nE 's/^version = "([^"]+)"$/\1/p' "$repo_dir/Cargo.toml")"
source_epoch="${SOURCE_DATE_EPOCH:-0}"

if [[ "$(uname -s)" != 'Linux' || "$release_arch" != 'x86_64' ]]; then
  echo 'Only Linux x86_64 release bundles are supported by this script' >&2
  exit 1
fi
if [[ ! "$release_version" =~ ^0\.[3456]\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]]; then
  echo 'Expected a single valid 0.3.x through 0.6.x version in Cargo.toml' >&2
  exit 1
fi
if [[ ! "$source_epoch" =~ ^[0-9]+$ ]]; then
  echo 'SOURCE_DATE_EPOCH must be a nonnegative integer' >&2
  exit 1
fi
if [[ ! -f "$binary_path" || ! -x "$binary_path" ]]; then
  echo "Release binary not found or not executable: $binary_path" >&2
  exit 1
fi
if [[ "$("$binary_path" --version)" != "RiceExplorer $release_version" ]]; then
  echo 'Binary --version does not match Cargo.toml; rebuild before packaging' >&2
  exit 1
fi

mkdir -p -- "$output_dir"
output_dir="$(cd -- "$output_dir" && pwd)"
package_name="rice-explorer-v${release_version}-linux-${release_arch}"
archive_name="$package_name.tar.gz"
if [[ -e "$output_dir/$archive_name" || -L "$output_dir/$archive_name" ]]; then
  echo "Release archive already exists; refusing to overwrite: $output_dir/$archive_name" >&2
  exit 1
fi
staging_root="$(mktemp -d)"
archive_temp="$(mktemp "$output_dir/.${archive_name}.tmp.XXXXXX")"
trap 'rm -rf -- "$staging_root"; rm -f -- "$archive_temp"' EXIT
bundle_dir="$staging_root/$package_name"

install -Dm755 -- "$binary_path" "$bundle_dir/rice-explorer"
install -Dm755 -- "$repo_dir/scripts/install-binary.sh" "$bundle_dir/install.sh"
install -Dm755 -- "$repo_dir/scripts/install_desktop_entry.sh" \
  "$bundle_dir/scripts/install_desktop_entry.sh"
install -Dm755 -- "$repo_dir/uninstall.sh" "$bundle_dir/uninstall.sh"
install -Dm644 -- "$repo_dir/assets/rice-explorer.desktop" \
  "$bundle_dir/assets/rice-explorer.desktop"
install -Dm644 -- "$repo_dir/assets/rice-explorer.svg" \
  "$bundle_dir/assets/rice-explorer.svg"
install -Dm644 -- "$repo_dir/LICENSE" "$bundle_dir/LICENSE"
install -Dm644 -- "$repo_dir/README.md" "$bundle_dir/README.md"
install -Dm644 -- "$repo_dir/CHANGELOG.md" "$bundle_dir/CHANGELOG.md"

# Keep deterministic ordering, ownership, timestamps and gzip headers. The
# release archive contains no user configuration or development credentials.
# Build in private staging first; a failed tar/gzip must not leave behind a
# deceptively named release artifact in the output directory.
tar --sort=name --format=posix --pax-option=delete=atime,delete=ctime \
  --owner=0 --group=0 --numeric-owner \
  --mtime="@$source_epoch" -C "$staging_root" -cf - "$package_name" \
  | gzip -n > "$archive_temp"

chmod 644 -- "$archive_temp"
# Linking a completed archive is atomic and fails on a competing existing name.
ln -- "$archive_temp" "$output_dir/$archive_name"

(cd -- "$output_dir" && sha256sum -- "$archive_name" > "$archive_name.sha256")
echo "$output_dir/$archive_name"
