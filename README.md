# RiceExplorer

**A Linux-first file explorer written in Rust, with a different personality for every desktop rice.**

[![Rust checks](https://github.com/CopyPasteUser/riceExplorer/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/CopyPasteUser/riceExplorer/actions/workflows/ci.yml)

> **Latest source:** [`main`](https://github.com/CopyPasteUser/riceExplorer/tree/main) — actively developed toward 1.0. The package currently identifies as **v0.2.0** (`Cargo.toml`); the **v0.3 file-operation improvements are implemented incrementally but have not passed the complete v0.3 release gate**. No version 1.0 release is claimed. [Development roadmap](ROADMAP.md) · [Changes](CHANGELOG.md) · [Latest build checks](https://github.com/CopyPasteUser/riceExplorer/actions/workflows/ci.yml)

RiceExplorer is designed as the file-management companion to [RiceBowl](https://github.com/CopyPasteUser/RiceBowl). Its renderer, filesystem operations and visual profiles are separate: switching from Windows 98 to Windows 7 or ChromeOS changes the chrome, geometry, controls and icons without replacing the file manager or disturbing an open folder.

**Built for structural customization.** RiceExplorer is not intended to be
one file manager with seven recolors. Its TOML shells can move whole navigation,
tab, menu and action components between regions; reorder sidebar modules and
move the preview pane. Windows XP can have a task-oriented side pane, Windows
10 a top command ribbon and a custom rice a bottom toolbar — all using the
same tested file-operation engine. More component regions and custom Rust
renderers remain part of the roadmap.

![RiceExplorer icon](assets/rice-explorer.svg)

## Implemented on `main` — v0.2 baseline + ongoing v0.3 work

- Native Rust GUI with GPU-accelerated drawing; no Electron, browser or persistent background daemon.
- Seven bundled visual profiles: `win98`, `xp-luna`, `win7`, `win10`, `frutiger-aero`, `chromeos` and `macos`.
- Configurable structural shells: classic menu/toolbar, XP task sidebar, Aero command bar, Windows 10 ribbon-like toolbar and modern condensed navigation. A universal actions menu exposes core file operations on every profile.
- Multiple tabs, history, path entry, home/parent navigation, bookmarks,
  mount-point navigation and a collapsible sidebar. Optional session restore
  remembers up to 32 tab locations and the active tab across restarts.
- List and icon layouts, local and bounded recursive filename search, hidden-file toggle and sorting by name, kind, size or modification time.
- Multiple selection (Ctrl/Shift/Ctrl+A), copy, cut, paste, duplicate, rename, new folder/file, symbolic links, desktop Trash and confirmed permanent deletion.
- **Paste with options…** in the shared Actions menu: choose **Keep both**
  (create a collision-free ` (copy)` name, also the normal Ctrl+V behavior)
  or **Skip existing** (leave destination files, folders and broken links
  untouched). Copy and Move use the same choice; skipped cut sources remain
  at their original location. A result message reports copied/moved and
  skipped counts. No replace/overwrite operation is currently offered.
- Batch rename for 2–256 selected entries (F2 or Rename): add a prefix/suffix,
  literal find/replace or configurable zero-padded sequence numbers. Preview
  every candidate before confirming. Prefix/suffix and numbering preserve raw
  non-UTF-8 filename bytes; find/replace requires UTF-8 names. Existing targets
  are rejected before any rename, with atomic no-overwrite behavior on Linux.
- Native GTK file clipboard interoperability (when `xclip` or `wl-clipboard` is installed); otherwise use the internal file clipboard.
- Cross-device moves with copy-first semantics and byte-for-byte verification, symlink-preserving copies, transfer progress and cancellation.
- Bounded transfer queue for up to 32 waiting copy requests with FIFO job IDs, a shared Transfers panel, active progress/cancel, removal of jobs before they run and 64 recent session results. The queue pauses after a failed/cancelled operation until explicitly resumed. It is in memory only and never replayed after a restart; moves and destructive requests do not wait automatically.
- Development preview: durable cross-device move journals and an always-visible, read-only warning for unfinished transfers, even when a rice hides its status bar. No automatic destructive recovery.
- File properties and editable Unix permission bits; external changes to folder contents are detected.
- On-demand preview pane for bounded image and text previews, decoded off the UI thread.
- Background folder scanning and file operations; superseded local scans and recursive searches are cooperatively cancelled and cannot overwrite current results. Virtualized file listing for large directories.
- Copy files into the open folder by dragging them from the desktop or another file manager.
- External TOML themes with configurable top/bottom tool regions, reorderable sidebar modules and left/right preview; persistent user settings. A single executable works with all rices.

**Most recent functional work:** a selectable collision policy for Copy/Move
with a **Paste with options…** dialog, worker-side skip decisions and regression
tests for mixed transfers and dangling target symlinks; plus worker-based
batch rename and opt-out tab-session restoration. The last
verified workflow checks Rust formatting, Clippy, unit tests, a release
build, CLI startup and Xvfb GUI startup for all seven skins. These are
development checks, **not** a production release or full Linux Mint acceptance.

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

On ordinary launches, RiceExplorer restores the previously opened tabs and
active location. **Explicit CLI folder arguments override the saved session.**
Use **Session → Restore tabs on startup** in the shared actions toolbar to opt
out and erase stored tab paths; the switch applies across all seven skins.
Sessions are limited to 32 absolute UTF-8 paths. Non-UTF-8 paths remain usable
but are not written to the TOML session; offline mounts are scanned in the
background and may show an error rather than blocking launch. Tab history,
search queries and pending file operations are **not** replayed on restart.

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

Theme schema 1 also supports a composable shell. Change actual **layout
structure**, not just colors, without recompiling. Put `schema_version = 1` with the top-level theme
fields (before `[palette]`), then add the shell section at the end:

```toml
[shell]
kind = "finder"
top = ["tabs", "navigation"]
bottom = ["command-bar"]
status_bar = true

[shell.sidebar]
position = "left"
width = 230
mode = "collapsible"
sections = ["tasks", "places", "favorites", "volumes"]

[shell.preview]
position = "right"
width = 310
```

`kind` selects `classic`, `luna`, `aero`, `ribbon`, `aqua`,
`chromeos`, `finder` or `modern`. `top` and `bottom` independently order the
`menu-bar`, `tabs`, `navigation` and `command-bar` modules. A module may appear
in **one region only**; navigation must remain present. If `top` is omitted,
moving a default component to `bottom` automatically removes it from the
default top region. Either region can be empty, but not both.

The sidebar supports `left`/`right`, width (120–420), `mode = "flat"` or
`mode = "collapsible"`, and ordered `tasks`, `places`, `favorites`, `volumes`
modules. Collapsible modules can be expanded and collapsed independently; XP
Luna enables this by default, while older themes remain flat. `tasks` provides real New Folder,
Copy, Cut, Paste, Rename, Trash and Properties controls; it does not duplicate
file-operation logic. `[shell.preview]` independently controls the preview
pane's `left`/`right` placement and width (210–520) when a preview is open.
`status_bar = false` hides only the optional status bar, never transfer or
recovery warnings. The main file view always remains available.

Older TOML themes without these fields retain their established layout.
Unsupported regions, duplicate widgets and invalid dimensions fail theme
validation. This is a **declarative built-in component system**, not arbitrary
runtime Rust code execution: a visual composer and compile-time custom
component registry still need to be built for version 1.0.

Per-user settings are stored in `~/.config/rice-explorer/config.toml` (or `$XDG_CONFIG_HOME/rice-explorer/config.toml`). You can customize bookmarks, current profile, hidden files, sidebar, view and sorting. Settings are written with a temporary file and rename.

### Shortcuts

| Keys | Action |
| --- | --- |
| Ctrl+T / Ctrl+W | New / close tab |
| Alt+Left / Alt+Right / Alt+Up | Back / forward / parent |
| Ctrl+L / Ctrl+F | Path / folder search |
| Ctrl+R / Ctrl+H | Refresh / hidden files |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut / paste selected files |
| Ctrl+Shift+C | Copy selected file paths as plain text |
| Ctrl+N / F2 / Delete | New folder / rename / move to Trash |
| Shift+Delete | Permanently delete (requires confirmation) |
| Ctrl+A / Ctrl+D / Alt+Enter | Select all / duplicate / properties |
| Space | Preview selected file |
| Ctrl-click / Shift-click | Extend selection / select range |

## Safety and performance

Current safeguards:

- Copies use private, hidden staging directories and publish complete entries
  with atomic no-overwrite rename. Existing targets get ` (copy)` suffixes;
  copying a folder into itself is rejected.
- Symlinks are copied as links, never traversed as directories; device files
  are rejected. A regular file swapped for a symlink while opening is refused.
- A source that changes identity, size, modification time or mode during copy
  fails validation before publication. Regular-file and directory access and
  modification timestamps are preserved where supported.
- Cross-filesystem moves verify each copied byte before publishing, then
  reverify published contents before deleting matching source entries.
  Extra verification I/O is intentional.
- Cancellation stops future work and cleans partial staging when possible,
  but does **not** undo already committed items. Deletion uses the desktop
  Trash unless permanent deletion is separately confirmed.
- Multi-item Rename, Trash, Delete and Link failures identify the stopped item
  and how many earlier items completed. Batch rename preflights all targets,
  but a competing filesystem writer can still cause a partial batch; it does
  not automatically undo earlier renames. Sequence numbers follow the stable
  selection order and appear after the prefix (`image-001-name`). Prefix/suffix
  applies to the entire original filename (including the extension). Permanent
  deletion reports missing selected items as errors, rather than false success.

**Important limitations:** Concurrent writers can still cause a partial move;
the complete published destination is retained for manual recovery if source
cleanup fails. ACLs, extended attributes, ownership, hardlink relationships,
some symlink timestamps and full race-free cleanup are not preserved or
guaranteed. **Do not use development builds for irreplaceable data.**

After a crash, a hidden `.rice-explorer-staging-*` directory may remain. Move
records live in `$XDG_STATE_HOME/rice-explorer/operations/` (default
`~/.local/state/rice-explorer/operations/`). RiceExplorer warns about unfinished
records and displays both source and destination paths. It will **never**
automatically delete, resume or overwrite based on these records. Inspect both
sides manually. The journal is not a full power-loss recovery engine. Filenames
are never interpolated into shell commands.

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
