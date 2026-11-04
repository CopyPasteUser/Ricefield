# RiceExplorer

**A Linux-first file explorer written in Rust, with a different personality for every desktop rice.**

[![Rust checks](https://github.com/CopyPasteUser/riceExplorer/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/CopyPasteUser/riceExplorer/actions/workflows/ci.yml)

> **Development preview: v0.3.0-alpha.4** — [`main`](https://github.com/CopyPasteUser/riceExplorer/tree/main). This is an **alpha**, not a safe replacement for Thunar with irreplaceable data. **Cross-filesystem Move now publishes a verified copy and keeps the original for manual cleanup; same-filesystem Move still renames.** Reliable restart recovery and physical Mint/USB acceptance remain outstanding. [Development roadmap](ROADMAP.md) · [Changes](CHANGELOG.md) · [Latest build checks](https://github.com/CopyPasteUser/riceExplorer/actions/workflows/ci.yml)

**Distribution:** An [automated Linux x86_64 preview bundle](RELEASE.md) can
be built without distributing Rust sources to every end user. The verified
bundle includes a user-only installer, icon and launcher; its CI checks do
not replace the outstanding real-device acceptance.

RiceExplorer is designed as the file-management companion to [RiceBowl](https://github.com/CopyPasteUser/RiceBowl). Its renderer, filesystem operations and visual profiles are separate: switching from Windows 98 to Windows 7 or ChromeOS changes the chrome, geometry, controls and icons without replacing the file manager or disturbing an open folder.

**0.4 groundwork (in development):** the CLI and address bar now share a
typed parser for local paths, `file://` URIs and validated SMB/SFTP/WebDAV/MTP
addresses. Remote addresses cannot yet be browsed: they fail clearly without
being misinterpreted as local paths. The Volumes sidebar now distinguishes
detected network mounts from local volumes, without treating mounted paths as
permission to mount or eject devices. GIO/GVfs connection handling comes next.

**Built for structural customization.** RiceExplorer is not intended to be
one file manager with seven recolors. Its TOML shells can move whole navigation,
tab, menu and action components between regions; reorder sidebar modules and
move the preview pane. Windows XP can have a task-oriented side pane, Windows
10 a top command ribbon and a custom rice a bottom toolbar — all using the
same tested file-operation engine. More component regions and custom Rust
renderers remain part of the roadmap.

![RiceExplorer icon](assets/rice-explorer.svg)

## Implemented on `main` — v0.3.0-alpha.4

- Native Rust GUI with GPU-accelerated drawing; no Electron, browser or persistent background daemon.
- Seven bundled visual profiles: `win98`, `xp-luna`, `win7`, `win10`, `frutiger-aero`, `chromeos` and `macos`.
- Configurable structural shells: classic menu/toolbar, XP task sidebar, Aero command bar, Windows 10 ribbon-like toolbar and modern condensed navigation. A universal actions menu exposes core file operations on every profile.
- Multiple tabs, history, path entry, home/parent navigation, bookmarks,
  mount-point navigation and a collapsible sidebar. Optional session restore
  remembers up to 32 tab locations and the active tab across restarts.
- List and icon layouts, local and bounded recursive filename search, hidden-file toggle and sorting by name, kind, size or modification time.
- Multiple selection (Ctrl/Shift/Ctrl+A), copy, cut, paste, duplicate, rename, new folder/file, symbolic links, desktop Trash and confirmed permanent deletion.
- **Paste with options…** in the shared Actions menu: an asynchronous,
  read-only preflight lists each source and intended target, counting
  collisions, automatic renames and skips before enabling Start. Choose
  **Keep both** (` (copy)` names, also normal Ctrl+V), **Skip existing**
  (skip without comparing), **Require no conflicts** (abort the entire batch
  if a target already exists), or **Skip only matching contents** (read-only
  comparison; fail the batch for different or unverifiable data). Content
  comparison has a 512 MiB / 10,000-entry safety budget, covers directories
  and symlink targets, and never deletes a skipped cut source. The worker
  repeats validation before making changes, rejects
  duplicate/overlapping sources and self-copy destinations, and never
  overwrites existing files. A concurrent filesystem change may still stop
  the operation safely. A successful comparison is only a momentary observation,
  not an atomic guarantee against other writers or permission to clean up
  a source. No replace/overwrite operation is currently offered.
- Batch rename for 2–256 selected entries (F2 or Rename): add a prefix/suffix,
  literal find/replace or configurable zero-padded sequence numbers. Preview
  every candidate before confirming. Prefix/suffix and numbering preserve raw
  non-UTF-8 filename bytes; find/replace requires UTF-8 names. Existing targets
  are rejected before any rename, with atomic no-overwrite behavior on Linux.
- Successful individual Rename operations provide a one-time **Undo rename**
  action in the shared Transfers history while the session remains open.
  Undo refuses to overwrite an occupied original name or rename an entry
  whose inode/metadata changed since the completed rename; it is not a
  persistent action log or a batch rollback.
- Legacy v3 cross-device Move journals record the checked staging payload's
  inode before publication and verifies that the published destination is
  that same object. The restart review shows this relationship separately
  from current source/destination metadata. Old v1/v2 records remain readable;
  a matching inode alone does **not** authorize automatic recovery or deletion.
- Native GTK file clipboard interoperability (when `xclip` or `wl-clipboard` is installed); otherwise use the internal file clipboard. Desktop copy/cut and paste run in background workers so a stuck helper cannot freeze the window. External payloads are capped at 4 MiB. Paste waits at most two seconds in the UI; on timeout **nothing is pasted** and a late result is ignored. Switching folders/tabs while a read is pending cancels that paste. Internal copies remain immediately available while desktop synchronization is pending.
- Cross-device Move now uses a staged, byte-verified **Copy with source retained**. It never automatically removes the original, and Transfers identifies every retained source for manual review and cleanup. Same-filesystem Move remains an atomic rename. Symlinks, progress and cancellation are supported.
- Staged file payloads and directories are synced before no-overwrite
  publication; the destination parent is synced after publication. If a
  filesystem cannot confirm the directory sync, the transfer reports an
  error and keeps the original source. This improves power-loss
  durability but does not guarantee all metadata preservation or automatic recovery.
- Trash and permanent deletion require explicit confirmation with a scrollable
  list of the actual selected paths. Multi-item requests
  now validate every selected path, reject duplicates/nested selections, and
  detect missing inputs **before removing anything**. Each attempted item has
  its own success/failure status in Transfers. A source changed after preflight
  is refused when its turn begins; Linux identity checks include modification
  and inode-change times. This is still **not** transactional: a later
  failure cannot restore an earlier deleted file, and concurrent directory
  modifications remain a risk. Prefer Trash when possible.
- Symbolic-link batches reserve distinct destination names before starting,
  never overwrite existing entries, and show per-link targets, failures and
  cancellations in Transfers. Earlier links remain after a partial failure.
- Bounded transfer queue for up to 32 waiting copy requests with FIFO job IDs, a shared Transfers panel, active progress/cancel, removal of jobs before they run and 64 recent session results. Active transfers show elapsed time, average transferred-byte rate and an approximate remaining time (after the first second and only when measurable); completed jobs retain their duration. Estimates may fluctuate on slow drives and during verification. **Copy and Move results expand into per-file statuses** (completed, skipped, failed, cancelled or not started), with actual target paths and error details on hover. A failed batch keeps the earlier completed items; no automatic rollback is implied. After a partially successful cut-and-paste **including a failed batch**, only unfinished sources remain on the internal clipboard for a deliberate retry; unchanged stale desktop cut contents are ignored. The queue pauses after a failed/cancelled operation until explicitly resumed. It is in memory only and never replayed after a restart; moves and destructive requests do not wait automatically.
- **Queue control:** open Transfers to manually pause or resume waiting Copy
  jobs. A running copy continues normally; pausing only prevents the next
  waiting job from starting. The pause is session-only and is not an Undo.
- Development preview: durable cross-device move journals and an always-visible,
  read-only warning for unfinished transfers, even when a rice hides its
  status bar. The recovery window checks whether the recorded source,
  destination and staging paths currently exist (including broken symlinks)
  and gives cautious guidance. New records also retain Unix source/published
  destination inode and metadata snapshots, so inspection can flag changed or
  replaced objects. The review now displays source/destination evidence
  separately and lets you browse their containing folders (or the staging
  folder) without following the recorded entry. When the original source's
  recorded identity still matches, an optional **separate Copy preview**
  defaults to Keep both and requires another confirmation; it does not resume
  the interrupted Move, delete a source or clear the record. Older records remain readable
  without identity evidence.
  Optional bounded read-only content comparison
  is available, but even matching bytes are **not** permission to delete
  anything. No automatic
  destructive recovery. Repeated rescans are coalesced while a filesystem
  inspection is active, so slow or disconnected mounts cannot spawn a growing
  group of duplicate scanners. If the 4,096-record journal limit is reached,
  validated **completed** records can be retired to free a slot; incomplete or
  unreadable recovery evidence is always retained. Current source-retaining
  cross-volume copies do not reserve new journal slots.
- File properties and editable Unix permission bits, loaded asynchronously so
  an unavailable mount cannot stall the entire UI. Slow requests are
  coalesced, stale results are ignored and permission edits remain disabled
  until the selected entry's metadata loads. External folder changes are
  detected.
- On-demand preview pane for bounded image and text previews, decoded off the UI thread.
- Background folder scanning and file operations; superseded local scans and recursive searches are cooperatively cancelled and cannot overwrite current results. Virtualized file listing for large directories.
- Copy files into the open folder by dragging them from the desktop or another file manager.
- External TOML themes with configurable top/bottom tool regions, reorderable sidebar modules and left/right preview; persistent user settings. A single executable works with all rices.

**Most recent functional work:** synced staging and destination directories;
read-only crash-recovery triage showing source, destination and staging status;
coalesced recovery rescans; whole-batch Trash/Delete preflight with per-item
outcomes; and transfer elapsed time, average speed and approximate ETA.
Safety hardening also rejects stale hardlink payloads
when a source inode changes mid-copy, and refuses journal records replaced
by symlinks, special files or different inodes while being scanned.
The metadata safety guard requires cross-device Move copies to match
source Unix owner/group, permissions and modification times, and (on Linux)
all readable extended-attribute names and values. Unsupported ownership or
ACL migration stops the Move instead of silently losing metadata.
Linux staging additionally preserves POSIX access/default ACLs and tries to
retain original uid/gid when permitted, without requiring extra dependencies.
Asynchronous Paste conflict previews, full-batch worker preflight and
*Require no conflicts* remain available. Detailed per-file results appear in
the Transfers window. The CI workflow checks
Rust formatting, Clippy, unit tests, a release
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
rice-explorer --version
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
  are rejected. A regular file swapped for a symlink or FIFO while opening is
  refused without following it or blocking on the FIFO. Verification also
  checks both opened file descriptors and their original paths for changes.
- Multiple names for the same regular-file inode **inside one copied directory
  tree** remain hardlinked in the staged destination. Separate top-level paste
  items and hardlinks outside the selected tree are not linked together. Unlike
  Copy, cross-device Move now refuses an inode with hardlink names outside the
  selected tree, or a hardlinked symlink, before staging; deleting its source
  would otherwise silently sever those relationships.
- On Linux, accessible `user.*` extended attributes are copied for files,
  directories and links. A destination that refuses one of these attributes
  aborts staged publication instead of silently discarding it. POSIX access
  and default ACLs are copied for regular files and directories; inherited
  staging ACLs are removed when absent on the original. Source uid/gid is
  also retained when permitted, including for symlinks. An ordinary Copy can
  still create a user-owned copy if it lacks permission to retain the owner;
  a cross-device Move refuses that mismatch instead of deleting the source.
  Security labels and capabilities are never replayed automatically.
- A source that changes identity, size, modification time or mode during copy
  fails validation before publication. Regular-file, directory and (on Linux)
  symbolic-link access and modification timestamps are preserved where supported.
- Cross-filesystem Move verifies copied bytes and supported metadata before
  atomic no-overwrite publication. A source-tree manifest and hardlink-scope
  check can refuse unsafe layouts. **No source entry is unlinked afterward**:
  concurrent writers make a recursive check-and-delete race-prone. Review
  the destination, then separately remove the source using the normal Trash
  action if appropriate. Transfers records this as "Copied — source kept",
  not as a finished rename. Privileged metadata may still be unsupported.
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

**Important limitations:** Concurrent writers can still cause a partial batch;
cross-volume sources remain available for manual cleanup even after a verified
copy. Security labels, file capabilities and other privileged
extended attributes are not automatically copied; owner/group changes may
require privileges, and hardlink relationships *between separately copied
source trees* are not guaranteed. Cross-device
Moves refuse copies that cannot retain the checked metadata. Linux POSIX ACLs
are migrated when supported, but not all filesystems implement them.
**Do not use development
builds for irreplaceable data.**

After a crash, a hidden `.rice-explorer-staging-*` directory may remain. **New
source-retaining cross-volume copies create no destructive Move journal.**
Older unfinished Move records still live in `$XDG_STATE_HOME/rice-explorer/operations/` (default
`~/.local/state/rice-explorer/operations/`). RiceExplorer warns about unfinished
records and displays source, destination and staging status. The recovery window
uses recorded inode/metadata snapshots for additional read-only warnings about
changed or replaced paths; identity matching is not proof of matching bytes,
authentic ownership or safety after reboot. Legacy v1 journals have no such
evidence. Their old 4,096-record capacity limit and journal creation lock
remain relevant only to compatibility/tests; unfinished evidence is never
pruned automatically.
The recovery window
also offers an **opt-in, read-only byte comparison** of source and destination,
including nested folders and symlink targets. It runs on a worker, can be
cancelled and stops after 512 MiB, 10,000 entries or 128 nesting levels; very
large trees and missing/inaccessible files require manual verification. A
matching snapshot **cannot prove that either path will stay unchanged**, and
never authorizes source deletion. RiceExplorer will **never** automatically
delete, resume or overwrite based on these records. The journal is not a full
power-loss recovery engine. Its browse buttons open the *parent* folders for
manual inspection; they do not treat any recorded path as a trusted symlink
target. The optional fresh Copy shortcut can duplicate the source to a new
collision-free name after a separate preflight and Start confirmation, but it
does not attest to source contents or replace crash recovery. Filenames are
never interpolated into shell commands.

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
