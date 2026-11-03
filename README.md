# RiceExplorer

**A Linux-first file explorer written in Rust, with a different personality for every desktop rice.**

RiceExplorer is designed as the file-management companion to [RiceBowl](https://github.com/CopyPasteUser/RiceBowl). Its renderer, filesystem operations and visual profiles are separate: switching from Windows 98 to Windows 7 or ChromeOS changes the chrome, geometry, controls and icons without replacing the file manager or disturbing an open folder.

![RiceExplorer icon](assets/rice-explorer.svg)

## Current features — v0.1

- Native Rust GUI with GPU-accelerated drawing; no Electron, browser or persistent background daemon.
- Seven bundled visual profiles: `win98`, `xp-luna`, `win7`, `win10`, `frutiger-aero`, `chromeos` and `macos`.
- Three interface families: classic menu/toolbar, Aero command bar, modern condensed navigation.
- Multiple tabs, history, path entry, home/parent navigation, bookmarks and a collapsible sidebar.
- List and icon layouts, per-folder search, hidden-file toggle and sorting by name, kind, size or modification time.
- Multiple selection (Ctrl/Shift), copy, cut, paste, rename, new folder/file and move to desktop Trash with confirmation.
- Background folder scanning and file operations; stale scan results cannot replace newer navigation results. Virtualized file listing for large directories.
- Copy files into the open folder by dragging them from the desktop or another file manager.
- External TOML themes and persistent user settings. A single executable works with all rices.

### Install on Linux Mint / Xfce

You need Rust/Cargo, `xdg-open` and the usual X11/OpenGL development libraries. On Ubuntu/Mint:

```bash
sudo apt install build-essential pkg-config libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libssl-dev libgl1-mesa-dev xdg-utils
git clone https://github.com/CopyPasteUser/riceExplorer.git
cd riceExplorer
bash install.sh
```

Or run directly from source: `cargo run --release -- --theme win7 "$HOME"`.

The installer places the executable at `~/.local/bin/rice-explorer`, a scalable icon under `~/.local/share/icons/hicolor/` and a `.desktop` launcher under `~/.local/share/applications/`. It does **not** change your default file manager or Xfce associations. It doesn't touch Thunar.

## Select a rice

```bash
rice-explorer --theme win7 ~/Downloads
rice-explorer --theme xp-luna ~/Documents
rice-explorer --theme win98
rice-explorer --theme frutiger-aero
rice-explorer --theme-file ~/.config/rice-explorer/my-rice.toml ~/Pictures
rice-explorer --list-themes
```

RiceBowl can launch the same binary with the active rice identifier. Suggested integration for its launcher: `rice-explorer --theme <profile> <folder>`. The optional file-manager override should be scoped to the active session; RiceBowl must restore its previous preference when leaving the rice. A system-wide `xdg-mime` override is *not* required.

| RiceBowl visual identity | Profile | Layout |
| --- | --- | --- |
| Windows 98 | `win98` | square classic |
| Windows XP Luna | `xp-luna` | blue classic |
| Windows 7 | `win7` | Aero command bar |
| Windows 10 | `win10` | flat modern |
| Frutiger Aero | `frutiger-aero` | aqua Aero |
| ChromeOS | `chromeos` | rounded modern |
| macOS | `macos` | Finder-inspired modern |

These profiles are *inspired by* their respective interface eras; they're not pixel-perfect OS clones, proprietary asset packs or system window decorations.

### Customize

Copy a file from [`themes/`](themes/) and edit its colors and metrics. Launch it with `--theme-file`. All colors use 6-digit `#RRGGBB`; layout is `classic`, `aero` or `modern`. The external profile controls window surfaces, selection, folder/file icons, radius, list density, tile width and sidebar width. Invalid custom themes produce an error and fall back to the bundled Windows 7 profile.

Per-user settings are stored in `~/.config/rice-explorer/config.toml` (or `$XDG_CONFIG_HOME/rice-explorer/config.toml`). You can customize bookmarks, current profile, hidden files, sidebar, view and sorting. Settings are written with a temporary file and rename.

### Shortcuts

| Keys | Action |
| --- | --- |
| Ctrl+T / Ctrl+W | New / close tab |
| Alt+Left / Alt+Right / Alt+Up | Back / forward / parent |
| Ctrl+L / Ctrl+F | Path / folder search |
| Ctrl+R / Ctrl+H | Refresh / hidden files |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut / paste selected files |
| Ctrl+N / F2 / Delete | New folder / rename / move to Trash |
| Ctrl-click / Shift-click | Extend selection / select range |

## Safety and performance

File copy creates new destination files without overwriting. Name collisions generate ` (copy)` suffixes; copying a directory into itself is rejected. Copying symlinks or special device files is intentionally blocked until safe native handling is implemented. Moving across filesystems currently reports an error rather than risking source deletion after a partial copy. Trash uses the platform Trash API, never permanent deletion. The application does not run shell commands with interpolated filenames.

Scanning, file copying and Trash operations run on worker threads. Views render only visible rows. The first release does not include thumbnails or recursive indexers, avoiding idle disk scanning on low-power laptops. Dragging files **out of** the window and desktop clipboard integration are planned, not yet implemented.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo run --release -- --theme win7
```

The included GitHub Actions workflow is configured to perform these checks and produce a Linux x86-64 executable artifact once the repository is published. See [DESIGN.md](DESIGN.md), [UX-CONTRACT.md](UX-CONTRACT.md) and [ROADMAP.md](ROADMAP.md).

License: MIT. Not affiliated with Microsoft, Apple or Google.
