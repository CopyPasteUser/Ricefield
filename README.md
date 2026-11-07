<div align="center">

# RiceField

**One Linux file manager. Seven unmistakable desktop styles.**

A fast, customizable **Rust + egui** file manager, standalone today and designed for the [RiceBowl desktop](https://github.com/CopyPasteUser/Ricebowl).

[Install](#install) · [Screenshot gallery](docs/screenshots/) · [User guide](docs/USAGE.md) · [1.0 readiness](docs/1.0-STATUS.md)

[![Rust checks](https://github.com/CopyPasteUser/Ricefield/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/CopyPasteUser/Ricefield/actions/workflows/ci.yml)

</div>

## Choose your desktop

Each rice changes the navigation, toolbar, density, color, icons and workflow—not just a coat of paint. All seven run the **same real file manager**.

| Windows 98 · Classic Explorer | Windows XP · Luna |
|:---:|:---:|
| [![Original Windows 98-inspired Classic shell](docs/screenshots/win98.png)](docs/screenshots/win98.png) | [![Windows XP Luna-inspired tasks pane](docs/screenshots/xp-luna.png)](docs/screenshots/xp-luna.png) |
| **Windows 7 · Aero** | **Windows 10 · Ribbon** |
| [![Windows 7-style Explorer shell](docs/screenshots/win7.png)](docs/screenshots/win7.png) | [![Windows 10-inspired Explorer Ribbon](docs/screenshots/win10.png)](docs/screenshots/win10.png) |
| **Frutiger Aero · Blue Skies** | **ChromeOS · Files** |
| [![Frutiger Aero-inspired aqua shell](docs/screenshots/frutiger-aero.png)](docs/screenshots/frutiger-aero.png) | [![ChromeOS-inspired Files shell](docs/screenshots/chromeos.png)](docs/screenshots/chromeos.png) |
| **macOS · Finder** | **Interactive views** |
| [![macOS Finder-inspired shell](docs/screenshots/macos.png)](docs/screenshots/macos.png) | [ChromeOS grid + More](docs/screenshots/README.md#chromeos--files-rice) · [Windows 98 Tools](docs/screenshots/README.md#windows-98--classic-explorer) · [All compact and split views](docs/screenshots/) |

The details are intentional: 98 has a gray menu and tree, XP a blue tasks pane, 7 an Aero command bar, 10 a Ribbon, Frutiger a glass shelf, ChromeOS a Files toolbar and macOS a Finder toolbar. Original vectors replace proprietary icons and logos. **Window decorations are controlled by RiceBowl or your Linux window manager**, not these file-manager themes.

## File management

- **Browse:** tabs, split panes, bookmarks, folder tree, list/grid views and session restore.
- **Organize:** copy, move, rename, cycle-safe batch rename, trash, file permissions, progress and conflict previews.
- **Find and inspect:** search, saved filters, thumbnails, Quick Look and ZIP/TAR/7z handling.
- **Reach storage:** SMB, SFTP, WebDAV and MTP discovery/browsing; optional credential storage; removable USB volume actions using UDisks2/Polkit.
- **Customize:** seven built-in rices plus user TOML files for palettes, spacing, navigation and layout.

**Release status: `1.0.0-rc.17` candidate.** Automated Rust tests and native Linux GUI screenshots pass, but the final physical Linux Mint/NAS/USB acceptance is still open. Keep your existing file manager installed.

## Install

Target: Linux x86_64 (Ubuntu 24.04 release builds support Linux Mint 22). The command is `ricefield`. Earlier release candidates used a different executable and settings path; back up your settings and remove obsolete launchers before upgrading.

```bash
sudo apt install build-essential pkg-config libxcb-render0-dev \
  libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libxkbcommon-x11-0 \
  libssl-dev libgl1-mesa-dev libarchive13t64 xdg-utils

git clone https://github.com/CopyPasteUser/Ricefield.git
cd Ricefield
bash install.sh
ricefield
```

Requires a current stable [Rust toolchain](https://rustup.rs/) when building from source. The installation is user-local and does not replace your default file manager. Release builds and checksums: [Releases](https://github.com/CopyPasteUser/Ricefield/releases) · [Release guide](RELEASE.md).

Try a specific rice:

```bash
ricefield --theme win98 ~/Documents
ricefield --theme xp-luna ~/Documents
ricefield --theme win7 ~/Downloads
ricefield --theme win10 ~/Downloads
ricefield --theme frutiger-aero ~/Pictures
ricefield --theme chromeos ~/Downloads
ricefield --theme macos
ricefield --theme-file /path/to/custom-rice.toml
```

## Safety and known limits

- Remote locations are **read-only**; downloads to a local folder are explicit, bounded and no-overwrite. Remote upload, rename and deletion remain disabled pending [atomicity guarantees](docs/REMOTE-WRITE-SAFETY.md).
- Cross-device moves preserve their sources after verified copies; cleanup requires a separate decision.
- Batch rename has guarded session-only Undo, not full crash recovery. Inspect any interrupted batch.
- USB mount, unmount and eject rely on UDisks2/Polkit; real-hardware acceptance and encrypted-volume unlock remain open.
- Recovery offers inspection, **not automatic rollback**. Test risky operations with disposable files.

[Detailed 1.0 status](docs/1.0-STATUS.md) · [Troubleshooting](docs/TROUBLESHOOTING.md) · [Roadmap](ROADMAP.md) · [Changelog](CHANGELOG.md)

## Develop

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

[Contributing](CONTRIBUTING.md) · [MIT License](LICENSE)

RiceField is not affiliated with Microsoft, Apple or Google. All included theme artwork is original.
