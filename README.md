# RiceExplorer

**A Linux-first file explorer written in Rust, with a different personality for every desktop rice.**

RiceExplorer is designed as the file-management companion to [RiceBowl](https://github.com/CopyPasteUser/RiceBowl). Its renderer, filesystem operations and visual profiles are separate: switching from Windows 98 to Windows 7 or ChromeOS changes the chrome, geometry, controls and icons without replacing the file manager or disturbing an open folder.

![RiceExplorer icon](assets/rice-explorer.svg)

## Current features — v0.2

- Native Rust GUI with GPU-accelerated drawing; no Electron, browser or persistent background daemon.
- Seven bundled visual profiles: `win98`, `xp-luna`, `win7`, `win10`, `frutiger-aero`, `chromeos` and `macos`.
- Three interface families: classic menu/toolbar, Aero command bar, modern condensed navigation. A universal actions menu exposes core file operations on every profile.
- Multiple tabs, history, path entry, home/parent navigation, bookmarks, mount-point navigation and a collapsible sidebar.
- List and icon layouts, local and bounded recursive filename search, hidden-file toggle and sorting by name, kind, size or modification time.
- Multiple selection (Ctrl/Shift/Ctrl+A), copy, cut, paste, duplicate, rename, new folder/file, symbolic links, desktop Trash and confirmed permanent deletion.
- Native GTK file clipboard interoperability (when `xclip` or `wl-clipboard` is installed); otherwise use the internal file clipboard.
- Cross-device moves with copy-first semantics and byte-for-byte verification, symlink-preserving copies, transfer progress and cancellation.
- File properties and editable Unix permission bits; external changes to folder contents are detected.
- On-demand preview pane for bounded image and text previews, decoded off the UI thread.
- Background folder scanning and file operations; stale scan results cannot replace newer navigation results. Virtualized file listing for large directories.
- Copy files into the open folder by dragging them from the desktop or another file manager.
- External TOML themes and persistent user settings. A single executable works with all rices.

### Install on Linux Mint / Xfce

You need a current Rust/Cargo toolchain, `xdg-open` and the usual X11/OpenGL development libraries. On Ubuntu/Mint:

```bash
sudo apt install build-essential pkg-config libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libxkbcommon-x11-0 libssl-dev libgl1-mesa-dev xdg-utils
git clone https://github.com/CopyPasteUser/riceExplorer.git
cd riceExplorer
bash install.sh
```

For file copy/paste interoperability with other file managers, install `xclip` on X11 or `wl-clipboard` on Wayland. Without them RiceExplorer still supports copy and paste between its own tabs.

Or run directly from source: `cargo run --release -- --theme win7 "$HOME"`.

The installer places the executable at `~/.local/bin/rice-explorer`, a scalable icon under `~/.local/share/icons/hicolor/` and a `.desktop` launcher under `~/.local/share/applications/`. It does **not** change your default file manager or Xfce associations. It doesn't touch Thunar.

To uninstall the application files, run `bash uninstall.sh` from the
repository. This leaves personal settings and themes intact; it does not
uninstall Thunar or change file associations. Re-running `bash install.sh`
updates the application from the checked-out source.

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

Theme schema 1 also supports a composable shell. Change the component order
without recompiling. Put `schema_version = 1` with the top-level theme
fields (before `[palette]`), then add the shell section at the end:

```toml
[shell]
kind = "finder"
top = ["tabs", "command-bar", "navigation"]
```

`kind` selects `classic`, `luna`, `aero`, `ribbon`, `aqua`,
`chromeos`, `finder` or `modern`. The `top` list controls order and
visibility of `menu-bar`, `tabs`, `navigation` and `command-bar`.
Older TOML themes without these fields retain their previous layout.
This is the first modular shell layer; a full visual editor, per-panel
layout and custom Rust component registry are still on the roadmap.

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
| Shift+Delete | Permanently delete (requires confirmation) |
| Ctrl+A / Ctrl+D / Alt+Enter | Select all / duplicate / properties |
| Space | Preview selected file |
| Ctrl-click / Shift-click | Extend selection / select range |

## Safety and performance

File copy creates new destination files without overwriting. Name collisions generate ` (copy)` suffixes; copying a directory into itself is rejected. Symbolic links are reproduced as links without following them; special device files are blocked. Across filesystems a move copies first, verifies every copied byte and only then removes the source (extra I/O is intentional). Cancelling a copy cleans its partial destination; a cancellation cannot undo earlier completed items. Normal deletion uses the desktop Trash; permanent deletion requires a separate confirmation. File names are never interpolated into shell commands.

Scanning, searching, copying and Trash operations run on worker threads. Views render only visible rows. Recursive search is bounded to protect small machines; it is not a full-text or indexed search. Image thumbnail previews are implemented; drag-out, advanced mount management, undo and full interactive desktop integration tests remain outstanding. Until these are verified on your Mint laptop, keep Thunar available as a fallback.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo run --release -- --theme win7
```

The included GitHub Actions workflow is configured to perform these checks and produce a Linux x86-64 executable artifact. Its GUI startup smoke is not a substitute for mouse, keyboard, storage, or data-integrity testing on Linux Mint. See [DESIGN.md](DESIGN.md), [UX-CONTRACT.md](UX-CONTRACT.md), [ROADMAP.md](ROADMAP.md) and [CHANGELOG.md](CHANGELOG.md).

License: MIT. Not affiliated with Microsoft, Apple or Google.
