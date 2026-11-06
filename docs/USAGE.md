# RiceField user guide

[README](../README.md) · [Troubleshooting](TROUBLESHOOTING.md) ·
[Current support boundary](1.0-STATUS.md)

## Navigation and sessions

Open a folder, use the address bar, or select a place, bookmark or mounted
volume in the sidebar. Tabs keep independent locations, selections and history.
**F3** opens split view; use **Focus** in either pane to direct shortcuts and
toolbar actions there. Right-click a tab to duplicate, reorder, close or reopen it.

Ordinary launches can restore up to 32 tab locations, bounded Back/Forward
history and 16 closed-tab locations. Explicit command-line locations override
the saved session. Disable **Session → Restore tabs on startup** to clear saved
locations. Queries, selections and pending file operations are not replayed.
Non-UTF-8 local paths remain usable but are excluded from saved TOML sessions.

Open a separate window with **Ctrl+Shift+N**. Its session does not replace the
primary window's saved tabs.

## Transfers and file actions

Use the Actions menu, context menu or keyboard shortcuts. Copy and Move offer
conflict previews; **Keep both** chooses a new name, **Skip existing** leaves
collisions untouched, and **Require no conflicts** refuses known collisions.
**Skip only matching contents** checks contents first and aborts on differences
or unreadable entries. Existing destinations are never silently replaced.

**Transfers** shows active progress, elapsed time, estimated speed/remaining
time when measurable, and per-file results. Up to 32 Copy jobs can wait in the
queue. Pause/Resume affects waiting jobs; the current transfer continues until
completion or cancellation. Failure/cancellation pauses the queue for review.
The queue and detailed per-item results are session-only; other mutations are
not silently queued. The most recent 64 result **summaries** survive a restart
in a private `transfer-history.toml` under the XDG state directory. Earlier
summaries are labeled read-only; they cannot replay an operation or enable Undo.
The primary window writes this ledger asynchronously. Secondary windows do not
replace it. Corrupt, unsupported or symlinked history files are preserved, not
silently rewritten. If another RiceField process owns the history lock, this
instance can read but not overwrite it.

A same-filesystem Move renames. A cross-filesystem Move publishes a verified
copy and **retains the original for separate manual cleanup**. Source metadata
that cannot be preserved can make verification fail. Review every item after a
partial batch; earlier completed items are not automatically rolled back.

Trash and permanent delete require confirmation of the selected paths.
Single completed renames can offer a guarded, session-only Undo in Transfers.
For multiple names, **F2** opens a batch-rename preview with prefix/suffix,
numbering and literal search/replace. Enable **Regular expression** to match
names using bounded Rust regex patterns with capture replacements such as `$1`
or `${name}`; regex/literal matching requires UTF-8 names. Every generated name
is validated before execution. Rename cycles and batch Undo are not implemented.

Drag into a local folder or between panes to copy without overwriting. For
outbound drag, the context action **Drag to another app…** uses an installed
`ripdrag` or `dragon` helper. Direct tile-to-other-app dragging is unavailable.

## Search, filters and duplicate candidates

- Type a name in Search; **Subfolders** includes nested folders.
- **Contents** opts into local UTF-8 text search. It skips symlinks, special
  files and files over 2 MiB, with file/byte/time budgets and partial-result notices.
- **Filters** combines type, size and modification-date presets. Expand
  **Custom pattern and ranges** for glob (`*`, `?`), case-insensitive Unicode
  regex, MiB bounds and age ranges. Invalid regex shows an inline error and no
  matches. **Clear filters** resets the filters.
- **Saved searches** explicitly stores up to 16 local folder/query/scope
  combinations. Ordinary transient queries are not saved automatically.
- **Actions → Find duplicate candidates in view…** compares visible local
  regular files by size and SHA-256. It is bounded, read-only and never deletes
  candidates. Review files before removing anything.

Remote Search matches filenames only. **Subfolders** opts into bounded remote
traversal; partial results and unavailable folders are labeled. Neither local
nor remote search starts an indexing daemon. Items hidden by filters are removed
from selection before subsequent file actions.

## Network shares and saved logins

Install GIO/GVfs support as listed in the [README](../README.md#installation).
Choose **Connect to network…** in Volumes or enter a supported URI:

```text
smb://server/share
sftp://example.org/home
davs://example.org/drive
mtp://[usb:001,008]/
```

Addresses must be valid URIs with percent-encoded spaces. Credentials belong in the connection prompt,
never in the URI or command line. Mounted system NFS paths use local navigation.

**Discover NAS on network…** searches advertisements through GVfs/Avahi. Select
**Browse shares** or **Sign in** for a discovered server. If the server does not
advertise, enter its URI once. Successful connections add URI-only favorites.

**Remember login in Linux keyring** is opt-in and saves credentials in Secret
Service after a successful mount. **Use saved login** retrieves them on demand;
**Forget saved login** removes them. There is no plaintext fallback or automatic
login. An unavailable/locked keyring or desktop session can prevent the operation.

Connected shares open as read-only tabs with history, refresh, bounded filename
search and small UTF-8 text previews. Click **Download** beside a remote file
or **Download folder** beside a directory to save its contents to the configured
local target directory (default `~/Downloads`).
Edit the **Download to** field using an absolute local folder path; valid
choices are saved for later sessions. Downloads use the destination selected
when each item was queued, even if this field is subsequently changed.
Up to 16 jobs can wait while one download runs. Remove individual waiting jobs,
pause the queue after the current job or resume it manually. After any transfer
error or cancellation, queued downloads pause to avoid repeated errors.
Waiting jobs are not restored or replayed after a restart.
The application creates the destination if needed,
streams no more than 8 GiB per file (20-minute timeout), and supports progress
and cancellation. Folder downloads additionally stop at 4,096 entries, 512
directories or 16 levels of depth. The 8 GiB and 20-minute limits apply to
the entire folder operation, not each contained file. It refuses an existing
file, directory or symlink with the same name,
and only publishes the final filename after the download and file sync finish.
Interrupted transfers do not appear as completed downloads. An abrupt process
termination can leave a private `.ricefield-download-*` staging directory in
the chosen target folder containing unfinished data; check before removing it. This does not
prove remote-source integrity if the server changes the file during reading.
Only entries that GIO reports as regular files or directories can be
downloaded. Reported symlinks and special network entries are refused,
including within a downloaded folder. A server-side type change during
transfer is not yet protected by a source-identity snapshot.
Folder downloads preserve filenames and file bytes, not remote ownership,
ACLs, extended attributes or server-side modification times; they are not
lossless NAS backups. Remote uploads, rename, deletion, full file opening
and binary previews are not implemented. Real NAS/offline/keyring/download
acceptance remains open.

## Archives and previews

Select local files/folders, then **Archive → Compress selected…**. Creation
supports ZIP, tar.gz, tar.xz, tar.zst and 7z through runtime libarchive. ZIP can
optionally use AES-256 encryption; RAR creation is not supported.

Select an archive, then **Archive → Browse / extract archive…** to list entries,
verify integrity or extract selected items into a new folder. Reading also
supports plain TAR and backend-supported RAR. Extraction rejects unsafe paths,
links and special files, applies resource limits, and stages output before
no-overwrite publication. Progress and cancellation are available.

Enter an archive password in the masked field and choose **Reload list** when
needed. Passwords remain in the open dialog/worker, not settings or keyring.
Visible ZIP filenames do not prove decryption succeeded: **Verify** and
**Extract** still require the correct password. Encrypted 7z/RAR and some ZIP
variants can fail even with the correct password; see the
[measured compatibility results](ARCHIVE-INTEROPERABILITY.md).

**Space** opens Quick Look; the Preview panel is separate. Image thumbnails use
an in-memory cache. Text/code previews are bounded; PDF first-page preview needs
`pdftoppm`, and media metadata needs `ffprobe`. Actual media playback uses Open.

Properties loads metadata in a worker. **Calculate folder size** and
**Calculate SHA-256** are explicit bounded analyses. Extended-attribute names
and ACL presence can be displayed; this is not a full ACL editor.

## Themes and shell composition

Choose a bundled profile with the theme picker or `--theme`:

| Profile | Visual direction |
| --- | --- |
| `win98` | Square classic |
| `xp-luna` | Blue classic with task groups |
| `win7` | Aero command bar |
| `win10` | Flat ribbon-like toolbar |
| `frutiger-aero` | Aqua Aero with lower command strip |
| `chromeos` | Rounded modern |
| `macos` | Finder-inspired |

**Customize theme…** imports TOML, edits source or visual controls, validates it
and saves a new private file with **Save & use theme**. Imported files and
bundled themes remain unchanged. CLI theme options take precedence for that
process. Visual edits can regenerate formatting and remove TOML comments.

The editor's **Shell composer · component layout** moves, hides and reorders
built-in toolbar components, chooses sidebar groups and preview placement, and
keeps Navigation present. File operations remain shared across shells.

For manual editing, copy an example from [`themes/`](../themes/), put
`schema_version = 1` among its top-level fields, and append a shell definition:

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

A component can appear in one region only; Navigation is required. Shell kinds
are `classic`, `luna`, `aero`, `ribbon`, `aqua`, `chromeos`, `finder` and `modern`.
Sidebar width is 120–420; preview width is 210–520. Both support left/right
placement. Hiding the optional status bar never hides transfer/recovery warnings.
Legacy themes retain their defaults. There is no arbitrary runtime Rust loader.

Launch with `--theme-file /absolute/path/to/rice.toml` for live reload. Updates
are checked every three seconds; rejected changes retain the last working theme.
The theme picker also offers **Reload external TOML**. Direct RiceBowl event sync
is not implemented; a launcher can pass `--theme <profile> <folder>`.

Preferences live at `$XDG_CONFIG_HOME/rice-explorer/config.toml`, defaulting to
`~/.config/rice-explorer/config.toml`; editor themes live in its `themes/`
subdirectory. Settings are privately written through temporary-file replacement.

## Linux desktop requests

`rice-explorer --file-manager-service` opts into FileManager1 on the session bus.
It opens supported folders, reveals local files and shows local properties.
It neither replaces nor queues behind an existing owner such as Thunar, and
installs no competing activation service. Active filters can hide requested
items. Remote item selection/properties fail explicitly.

**Actions → Open With…** discovers registered local MIME handlers through GIO
and launches the selected desktop application. Portal integration remains open.

## Keyboard reference

| Shortcut | Action |
| --- | --- |
| Ctrl+T / Ctrl+W / Ctrl+Shift+T | New / close / reopen tab |
| Ctrl+PageUp / Ctrl+PageDown | Previous / next tab |
| Ctrl+Shift+PageUp / Ctrl+Shift+PageDown | Reorder active tab |
| F3 / Ctrl+Shift+N | Split view / separate window |
| Alt+Left / Alt+Right / Alt+Up | Back / forward / parent |
| Ctrl+L / Ctrl+F | Address bar / search |
| Ctrl+R / Ctrl+H | Refresh / hidden files |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut / paste |
| Ctrl+Shift+C | Copy paths as plain text |
| Ctrl+N / F2 | New folder / rename or batch rename |
| Delete / Shift+Delete | Trash / confirmed permanent delete |
| Ctrl+A / Ctrl+D / Alt+Enter | Select all / duplicate / properties |
| Space / Escape | Toggle Quick Look / dismiss the active preview or dialog |
| Ctrl-click / Shift-click | Toggle selection / select a range |

Global file shortcuts do not replace text-field clipboard behavior or execute
behind confirmation/login/recovery dialogs.
