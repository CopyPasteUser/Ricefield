# RiceExplorer

**A native Linux file manager that fits your desktop rice.**

[![Rust checks](https://github.com/CopyPasteUser/riceExplorer/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/CopyPasteUser/riceExplorer/actions/workflows/ci.yml)

Built with Rust and egui, RiceExplorer combines everyday file management with
seven visual shells and a theme editor. It is the file-management companion to
[RiceBowl](https://github.com/CopyPasteUser/RiceBowl).

[Install](#installation) · [User guide](docs/USAGE.md) ·
[Release status](docs/1.0-STATUS.md) · [Changelog](CHANGELOG.md)

> **Current version: 1.0.0-rc.1.** This is a release candidate; stable 1.0
> acceptance is still open. Keep Thunar available and use disposable data when
> testing storage, network and recovery workflows.

![RiceExplorer with the Windows 7 shell](docs/screenshots/win7.png)

## What you can do

- **Browse your files:** tabs, split view, folder tree, bookmarks, list/grid
  layouts and optional session restore.
- **Manage local files:** create, copy, move, rename, batch rename, link and
  trash, with transfer progress, conflict previews and per-file results.
- **Find and inspect:** filename/content search, glob and regex filters,
  saved searches, duplicate candidates, thumbnails and Quick Look.
- **Work with archives:** browse, verify, create and extract supported local
  ZIP, TAR and 7z formats, including password-protected ZIP.
- **Browse network shares:** read-only SMB, SFTP, WebDAV and MTP tabs through
  GIO/GVfs, NAS discovery and optional login storage in the Linux keyring.
- **Choose your shell:** Windows 98, XP Luna, Windows 7, Windows 10,
  Frutiger Aero, ChromeOS or macOS-inspired layouts. Edit colors, density,
  toolbars and panel placement without changing file-operation behavior.

## Installation

The distribution target is **Linux x86_64**, built on Ubuntu 24.04 for the
Linux Mint 22 generation. Physical Mint X11/Wayland acceptance is still pending.

### Build from source

Install a current stable [Rust toolchain](https://rustup.rs/), then install the
build and runtime dependencies on Ubuntu 24.04 / Mint 22:

```bash
sudo apt install build-essential pkg-config libxcb-render0-dev \
  libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libxkbcommon-x11-0 \
  libssl-dev libgl1-mesa-dev libarchive13t64 xdg-utils

git clone https://github.com/CopyPasteUser/riceExplorer.git
cd riceExplorer
bash install.sh
```

Launch **RiceExplorer** from your application menu, or run
`~/.local/bin/rice-explorer`. Installation is user-local and leaves your default
file manager unchanged. Re-running `bash install.sh` updates the checked-out
version; `bash uninstall.sh` removes the application while keeping preferences.

### Install a binary bundle

Published bundles are listed under [Releases](https://github.com/CopyPasteUser/riceExplorer/releases).
An unreleased candidate can be built from source above. For a downloaded bundle,
verify its SHA-256 manifest, extract it and run `bash install.sh` inside the
extracted folder. Rust is not required. See [release and bundle instructions](RELEASE.md).

<details>
<summary>Optional desktop features</summary>

| Feature | Ubuntu / Mint package or tool |
| --- | --- |
| Clipboard with other file managers | `xclip` on X11; `wl-clipboard` on Wayland |
| Network browsing and NAS discovery | `libglib2.0-bin`, `gvfs-backends`, `avahi-utils` |
| Saved logins in Linux Secret Service | `libsecret-tools` |
| PDF first-page preview | `poppler-utils` |
| Audio/video metadata | `ffmpeg` (`ffprobe`) |
| Native drag-out helper | Installed `ripdrag` or `dragon` |

Without external clipboard tools, copying between RiceExplorer tabs still works.

</details>

## Get started

```bash
rice-explorer ~/Downloads
rice-explorer --theme xp-luna ~/Documents
rice-explorer --theme macos
rice-explorer --theme-file /path/to/my-rice.toml
```

Use the theme picker to switch shells, or choose **Customize theme…** to edit
one and save your own. Bundled profile IDs are `win98`, `xp-luna`, `win7`,
`win10`, `frutiger-aero`, `chromeos` and `macos`.

| Shortcut | Action |
| --- | --- |
| F3 | Toggle split view |
| Ctrl+T / Ctrl+W / Ctrl+Shift+T | New / close / reopen tab |
| Ctrl+L / Ctrl+F | Address bar / search |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut / paste |
| Ctrl+N / F2 / Delete | New folder / rename / trash |
| Space / Alt+Enter | Quick Look / properties |

The [user guide](docs/USAGE.md) covers network connections, archives, filters,
transfer controls, theme composition and the complete shortcut list.

## Know the current limits

- **Cross-filesystem Move keeps the original.** RiceExplorer publishes a
  verified copy; you review and clean up the source separately. Same-filesystem
  Move uses a rename.
- **Remote tabs are read-only.** Remote writes and downloads are not implemented.
  Archive encryption support also depends on the installed libarchive backend.
- **Recovery is a review workflow.** Journal inspection and content comparison
  do not provide automatic resume, rollback or source deletion. Completed parts
  of a failed batch may remain.

See [candidate scope and remaining 1.0 gates](docs/1.0-STATUS.md) for the complete
support boundary, lab results and device checklist. For mounting, clipboard or
launcher problems, start with [troubleshooting](docs/TROUBLESHOOTING.md).

## Development

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo run --locked --release -- --theme win7
```

CI also exercises native GUI flows, a private FileManager1 session bus,
installation/rollback and disposable performance fixtures. Exact commands are
in the [verification guide](docs/1.0-STATUS.md#repeatable-automated-checks).

[Contributing](CONTRIBUTING.md) · [Roadmap](ROADMAP.md) ·
[Release process](RELEASE.md) · [Security](SECURITY.md)

Licensed under [MIT](LICENSE). The visual profiles are independently drawn,
OS-inspired designs; the project is not affiliated with Microsoft, Apple or Google.
