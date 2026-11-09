<div align="center">

# RiceField

**One Linux file manager. Seven distinct desktop styles.**

A customizable **Rust + egui** file manager for Linux, usable standalone or alongside [RiceBowl](https://github.com/CopyPasteUser/Ricebowl).

[Install](#install) · [Screenshot gallery](docs/screenshots/README.md) · [User guide](docs/USAGE.md) · [1.0 status](docs/1.0-STATUS.md)

[![Rust checks](https://github.com/CopyPasteUser/Ricefield/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/CopyPasteUser/Ricefield/actions/workflows/ci.yml)

</div>

| Windows 7 · Aero | Frutiger Aero |
| :---: | :---: |
| [![Windows 7 shell](docs/screenshots/win7.png)](docs/screenshots/win7.png) | [![Frutiger Aero shell](docs/screenshots/frutiger-aero.png)](docs/screenshots/frutiger-aero.png) |

[View all seven styles](docs/screenshots/README.md): Windows 98, XP Luna, Windows 7, Windows 10, Frutiger Aero, ChromeOS and macOS. The window manager controls the window decorations.

## Features

- **Everyday file management:** tabs, split view, bookmarks, search, previews, drag-and-drop, copy/move/rename/trash and `Open With`.
- **Guarded operations:** no-overwrite conflict previews, transfer history, selective session Undo and recovery diagnostics.
- **Archives:** browse and extract ZIP, TAR, 7z and RAR with libarchive.
- **Network and devices:** browse SMB, SFTP, WebDAV and MTP; manage removable media through UDisks2/Polkit.
- **Customization:** seven built-in themes, custom TOML appearances and optional RiceBowl RiceProfile integration.

**Status:** `1.0.0-rc.21` is a release candidate, **not stable 1.0**. Physical desktop, USB and NAS acceptance remains open. Keep your existing file manager installed.

## Install

Linux x86_64 (Ubuntu 24.04 / Linux Mint 22); building from source requires a current [Rust toolchain](https://rustup.rs/).

```bash
sudo apt install build-essential pkg-config libxcb-render0-dev \
  libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libxkbcommon-x11-0 \
  libssl-dev libgl1-mesa-dev libarchive13t64 xdg-utils curl

git clone https://github.com/CopyPasteUser/Ricefield.git
cd Ricefield
bash install.sh
ricefield
```

Installation is user-local and creates an application-menu entry; it does not replace your default file manager. [Release builds](https://github.com/CopyPasteUser/Ricefield/releases) · [Release guide](RELEASE.md).

Choose a style or validate a custom theme:

```bash
ricefield --theme xp-luna ~/Documents
ricefield --theme frutiger-aero ~/Pictures
ricefield --theme macos
ricefield --check-theme-file /path/to/custom-rice.toml
```

## Safety and documentation

Local Undo covers **only eligible operations**; copies, deletions and cross-volume moves are not automatically reversible. Remote browsing is read-only by default. Verified WebDAV, SFTP and signed SMB servers may support **guarded single-file uploads**, but not remote rename/delete; interrupted uploads require manual review. Cross-device moves preserve the source after copying.

[Remote capability limits](docs/REMOTE-1.0-SCOPE.md) · [Recovery runbook](docs/REMOTE-RECOVERY-RUNBOOK.md) · [Owner acceptance checklist](docs/OWNER-ACCEPTANCE-1.0.md) · [Troubleshooting](docs/TROUBLESHOOTING.md) · [Roadmap](ROADMAP.md) · [Changelog](CHANGELOG.md)

## Development

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

[Contributing](CONTRIBUTING.md) · [MIT License](LICENSE)

RiceField is independent of Microsoft, Apple and Google; included artwork is original.
