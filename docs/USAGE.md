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
**Paste with options** also allows an exact target name for each copied or
moved source. An occupied name aborts the whole batch before any transfer;
moving to a different name in the same folder performs a guarded rename.

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

After a failed or cancelled local Copy/Move batch, **Review unstarted items**
can reopen Paste with options for items that never began. Review the current
paths and choices, then explicitly start a new transfer if safe. Failed or
already processed items are excluded because their destination may require
manual inspection. This button is session-only and cannot replay history.
Each per-file row now offers **Inspect**, which opens the recorded target's
containing folder in a new tab (or the source folder when no target is known).
Inspect never retries, deletes or changes a result; a failed publish may have
left both copies, and the user must examine them before deciding what to do.

A same-filesystem Move renames. A cross-filesystem Move publishes a verified
copy and **retains the original for separate manual cleanup**. Source metadata
that cannot be preserved can make verification fail. Review every item after a
partial batch; earlier completed items are not automatically rolled back.

Trash and permanent delete require confirmation of the selected paths.
Single completed renames and entirely successful batch renames can offer a
guarded, session-only **Undo** in Transfers. A fully successful, single-file
Copy can be undone by sending its unchanged result to desktop Trash; a
same-filesystem single-file Move can be returned to its unoccupied original
name; a single Trash entry can be restored if its exact inode and Trash
metadata are still present. Restoring from Trash on Linux uses an atomic
no-overwrite rename, not the backend's ordinary rename. These three Undo
operations refuse changed or replaced objects, hardlinked files and
collisions; failed or partial batches, directories and cross-device Moves
cannot be undone automatically. No Undo is offered after restart; previous
session history is display-only. When Undo fails, use **Inspect** before
retrying rather than assuming either location is safe to remove.
For multiple names, **F2** opens a batch-rename preview with prefix/suffix,
numbering and literal search/replace. Enable **Regular expression** to match
names using bounded Rust regex patterns with capture replacements such as `$1`
or `${name}`; regex/literal matching requires UTF-8 names. Every generated name
is validated before execution. Durable batch Undo is not implemented.
Batch rename can exchange names belonging to the same selected set without
discarding either entry, including cycles and hardlinks. A collision with any
unselected destination is refused. Ordinary errors attempt a guarded rollback;
after a crash or concurrent external edit, inspect the displayed partial names
manually. Undoing a *completed batch* is available only during the same
session, and only if every post-rename object is unchanged and every original
destination name is free or part of that same undo batch. In earlier sessions
the restored history is read-only; a batch cannot be automatically replayed.

Select one or more local files/folders, then drag them onto another folder,
breadcrumb, folder-tree entry, local tab, or split pane. Dragging **copies**;
hold **Shift** while releasing to **move** (same-volume rename, guarded
cross-volume copy with the original retained). The destination gets a colored
outline and an operation hint. Dropping onto the original directory does
nothing. Conflicts use new names rather than overwriting existing files.
Press **Esc** to cancel. Dragging files **into** RiceField from another app
copies them to the local folder under the pointer, or the focused folder if
the desktop does not report a pointer position. Drops onto non-folder controls
are ignored. Remote tabs remain read-only.

To drag **out** to another application, select up to 128 local files/folders
and press **Ctrl+Shift+D**, or choose **Drag to another app…** from the
file's context menu. An installed `ripdrag` or `dragon` starts a native
drag-source window; if neither is installed, RiceField explains the
limitation and you can use **Ctrl+C** and paste into the target app instead.
Remote files must first be downloaded. Direct in-process drag-out from a
file tile is not supported by this version of eframe/winit.

On Wayland with XWayland available (`WAYLAND_DISPLAY` and `DISPLAY` set),
launch `ricefield --xwayland-dnd` to explicitly choose the native X11 XDND
backend for incoming file drops. This is optional because XWayland may alter
display scaling, input methods or other desktop integration. Use the normal
Wayland launch if that works better for your session.

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
Choose **Connect to NAS** from the sidebar, then enter the server's IP address or
hostname (for example, `192.168.178.66` or `nas.local`). Enter a share name
such as `medien` to connect directly, or leave it blank and choose
**Browse shares** to select from the NAS. **Find NAS on network** uses existing
GVfs/Avahi advertisements; it never scans the network or logs in automatically.
A NAS that does not advertise can always be added manually. You can paste
`smb://server/share` or a Windows-style `\\\\server\\share` path in the
server field. Spaces in the share name are encoded automatically.

The **Other protocols** tab retains full-URI entry for SFTP, WebDAV and MTP:

```text
sftp://example.org/home
davs://example.org/drive
mtp://[usb:001,008]/
```

Enter a username and password only when a particular share needs them.
The domain/workgroup and optional Linux keyring controls live under
**Advanced login options**. Credentials never appear in URIs or logs.
Successful connections save URI-only favorites. **Places** lists the usual local folders directly, including Desktop, Music,
Videos and Filesystem. The separate **Locations** section (drives and network)
starts collapsed to keep the left sidebar compact. Expand it when needed for
mounted volumes or connecting to NAS; saved connections and inactive devices
have their own expandable groups.

**Remember login in Linux keyring** is opt-in and saves credentials in Secret
Service after a successful mount. **Use saved login** retrieves them on demand;
**Forget saved login** removes them. There is no plaintext fallback or automatic
login. An unavailable/locked keyring or desktop session can prevent the operation.

Connected shares open as predominantly read-only tabs with breadcrumbs,
sortable selectable files, history, refresh, bounded filename search and
text/binary prefix previews. Only the first 32 KiB are streamed for
preview; large text is marked truncated, and binary bytes appear as a safe
256-byte hex/ASCII sample. These previews are not source-integrity checks.
Click **Download** beside a remote file
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
lossless NAS backups. For **WebDAV only**, select **Upload file…** in a remote
tab, enter an absolute local regular-file path (prefilled from one selected
local tab item when available), and supply a username/password or an exactly
matching saved Secret Service login. RiceField first runs live server
no-overwrite probes, copies the local source to a private snapshot, uploads a
hidden conditional stage, compares every byte via GET and asks the server to
publish atomically with `MOVE Overwrite: F`. The existing target must be
refused with HTTP 412. Private `.ricefield-upload-*` stages may remain on
server failures or crashes; inspect them manually. There is no automatic
overwrite fallback, folder upload, remote rename or remote delete. SMB/SFTP/
MTP writes remain unavailable, and **real NAS/offline/interrupt acceptance
is required before 1.0**. The client also needs `curl` installed; HTTPS uses
the normal system CA trust, never an insecure-TLS bypass. For `dav://` plain
HTTP outside loopback, the user must explicitly acknowledge that passwords
and file contents may cross the network unencrypted; use `davs://` whenever
possible.

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
an in-memory cache. Text/code previews are bounded; binary files show up to
256 bytes in a read-only hex/ASCII view without executing or decoding their
contents. Larger binary files are explicitly marked truncated. This applies
to local files, not network URI previews. PDF first-page preview needs
`pdftoppm`, and media metadata needs `ffprobe`. Actual media playback uses Open.

Properties loads metadata in a worker and displays human-readable KiB/MiB/GiB
sizes. Directories do not misleadingly show their inode size; **Calculate folder
size** and **Calculate SHA-256** are explicit bounded analyses. Extended-attribute names
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

Preferences live at `$XDG_CONFIG_HOME/ricefield/config.toml`, defaulting to
`~/.config/ricefield/config.toml`; editor themes live in its `themes/`
subdirectory. Settings are privately written through temporary-file replacement.

## Linux desktop requests

`ricefield --file-manager-service` opts into FileManager1 on the session bus.
It opens supported folders, reveals local files and shows local properties.
It neither replaces nor queues behind an existing owner such as Thunar, and
installs no competing activation service. Active filters can hide requested
items. Remote item selection/properties fail explicitly.

**Actions → Open With…** discovers registered local MIME handlers through GIO
and launches the selected desktop application. Portal integration remains open.

## Interrupted network downloads

In a remote tab, select the local **Download to** directory and click
**Inspect interrupted downloads**. RiceField scans that folder on a worker
thread for up to 64 private `.ricefield-download-*` staging directories and
offers **Inspect folder** for manual review. It does not read their contents,
remove them or resume a download. A matching folder may belong to a download
still running in another RiceField instance. No result authorizes deleting or
overwriting either the local or remote source. If the folder contains more
than 8,192 entries, inspection stops with an error rather than presenting an
incomplete scan as exhaustive. The normal file operation journal is separate.

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
