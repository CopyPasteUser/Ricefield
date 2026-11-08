# RiceField UX contract

## Scope

Linux desktop file management for RiceBowl users. v0.1 interface English; runtime GUI built in egui. Visual language is owned by [DESIGN.md](DESIGN.md) and `themes/*.toml`. The filesystem module is the canonical owner for file mutation.

## Navigation and selection

- Tabs preserve their own path, back/forward history, search query, selection and pending scan generation.
- Tab actions use one skin-independent dispatcher: a tab context menu can
  duplicate, move or close the chosen tab and reopen the last closed tab.
  Ctrl+PageUp/PageDown cycles tabs, Ctrl+Shift+PageUp/PageDown reorders the
  active tab, Ctrl+T opens a new tab, Ctrl+W closes it and Ctrl+Shift+T reopens
  a closed one. Reordering keeps the same logical tab active. Duplication and
  reopen copy local/remote address and navigation/search history but create
  independent scan and preview cancellation tokens, and never clone stale
  entry lists or selections. The closed-tab stack holds at most 16 snapshots
  in memory; it is not serialized and closing the only tab goes Home.
- Network folders can open in a new tab through discovery, the selected-folder
  toolbar action or a row context menu. The new tab starts directly at the
  validated remote URI, with independent scan/preview tokens and no visit to
  its local fallback path; read-only restrictions remain identical.
- The session snapshot keeps at most 32 absolute, UTF-8 tab paths and one
  active index. An explicit launch path overrides restoration, and opting out
  clears the stored paths. Unavailable mounts are handled by asynchronous
  folder scans; never synchronously probe them during session hydration.
  Search/history/selection and filesystem mutations are never replayed.
- Each tab owns a cooperative scan cancellation token. Navigation, refresh,
  closure and changes to recursive search queries invalidate superseded
  workers; cancelled/queued stale results must never replace current entries.
- Address/CLI file-to-parent resolution and symlink-directory classification
  run on scan workers, never through synchronous path probes in widgets.
  Sidebar place availability is refreshed with the background volume snapshot.
- The optional `--file-manager-service` uses native FileManager1 D-Bus requests.
  It never replaces or queues behind an existing owner. Requests are bounded,
  credential-free and deferred while a confirmation or login is open.
  Local ShowItems groups requested selections by parent folder, retaining only
  visible selections; filters may hide an item. Properties are presented one
  at a time. Remote ShowFolders is read-only; remote item selection/properties
  fail explicitly rather than invoking the local filesystem engine.
- Filename regex is an optional case-insensitive local filter alongside glob.
  Compilation has a 512-byte pattern / 1 MiB automaton budget, matching has no
  backtracking, and invalid syntax shows an inline error with zero matches.
  Clearing filters restores results and removes the regex from preferences.
- Filters is an editable popup: internal clicks keep it open; Escape or an
  outside click closes it. Custom options and text inputs remain operable.
- Entering a new location clears local search and selection, but not bookmarks or preferences.
- Changing local type/size/date filters recomputes the visible entries in
  memory; any selected path no longer visible is deselected before file
  operations. Size presets apply only to regular files (not directories or
  symlink lengths); modified-date presets use the entry's reported timestamp.
- Sorting always groups directories before files. Filters operate only on the current folder; no unrequested recursive search/indexing.
- Shift selects a contiguous visible range; Ctrl toggles one item. Navigation history is independent of filesystem mutation history.
- Keyboard shortcuts do not copy/cut/paste files while a text field has focus.
- Shortcut modifiers come from the individual press event. A chord pressed
  and released between frames must still work, and opening a dialog stops
  dispatch of subsequent global shortcuts from that frame.
- Global navigation/file shortcuts are suspended while network login,
  mount confirmation, network discovery or recovery review windows are open;
  typing in those surfaces cannot create folders or switch file tabs behind
  the dialog. Ordinary address/search focus still permits Ctrl+N.

## Mutations

| Action | Pending | Success | Failure |
| --- | --- | --- | --- |
| New file/folder | Background worker | Refresh all tabs; status message | Keep existing folder; error in status |
| Rename | Background worker | Refresh tabs; status message; session-only Undo in Transfers if a post-rename inode snapshot is captured | Existing target remains; error in status |
| Batch rename | Snapshot selection, preview all candidate names, preflight targets on worker | Rename each item without overwriting | Stop with failed item and completed count; no silent rollback claim |
| Copy | Background worker | Unique new destination; status | Partial new output cleaned when possible; source intact |
| Move | Background worker | Rename within volume; cross-volume verified copy with source kept | Original kept on cross-volume success; errors and per-item status explicit |
| Trash | Explicit confirmation, then worker | Platform Trash; status | Report error; refresh tabs to show partial state |

Only one mutation worker runs at a time; folder reads may proceed concurrently. Copy requests arriving during another mutation may enter a bounded 32-item FIFO queue with stable IDs. Queued jobs run after a successful active worker; after failure or cancellation they pause until explicit Resume, so further transfers do not cascade into a known failure. Each waiting job can be removed before execution without touching files. Other mutations are not delayed automatically, to avoid executing stale destructive requests. There is no fake optimistic success. The Trash confirmation is deliberately conservative even though desktop Trash is recoverable.

The shared Transfers dialog also permits manual Pause / Resume of **waiting**
Copy jobs. Any current transfer continues to completion or explicit
cancellation; the pause does not suspend file I/O, lock changing sources,
persist across restarts, or provide Undo.

Completed single-item Renames may carry a one-time Undo action in the same
session's Transfers history. Undo dispatches through the canonical filesystem
worker, checks the post-rename inode/metadata snapshot and uses atomic
no-overwrite rename to restore the original raw filename. If either source
changed or the original name is occupied, Undo fails without replacing files;
the control remains available for a deliberate retry. The action is not
persisted across restarts and is not a general rollback guarantee.

Cross-device Move (implemented as source-retaining Copy) refuses a selected source tree with hardlinks to names
outside the selected tree, or a hardlinked symlink, before it stages output.
Copy may produce independent inodes; the conservative Move pathway does not silently sever surviving
hardlink relationships. Neither check provides atomic protection from an
external writer changing source link counts after the preflight.

After verifying and publishing a cross-device Move's destination, RiceField
**must not automatically unlink the source**. Transfers marks it "Copied —
source kept", and the completion text instructs manual review and cleanup.
The associated cut clipboard intent is consumed so an old Cut does not
silently paste the same verified copy twice. Same-filesystem Move retains its
atomic rename behavior.

Cross-device Copy journals save the verified staged object's inode before
publication. If publishing races with another writer, is cancelled or fails
after verification, RiceField retains that verified staging data for review;
unverified/partial staging is disposable on ordinary errors. Recovery can
compare the original with the staged payload separately from the destination.
On Linux, an explicitly confirmed, still-identical file, symlink or bounded
directory stage may be published only to an unoccupied target with
descriptor-relative atomic `RENAME_NOREPLACE`, durable directory sync and a
second bounded comparison. Directories additionally require exact nested
permissions, owner/group, timestamps, xattrs and in-tree hardlink topology.
The original stays untouched, and stale, changed, colliding or unverifiable
cases fail closed. A successful publication retires the journal; no automatic
recovery, source deletion, overwriting or general rollback exists.
Older journal versions without staged inode evidence remain inspectable and
explicitly marked as unproven.

Copy/cut immediately updates the internal file clipboard and synchronizes
the desktop clipboard in a worker. Outstanding desktop writes coalesce to the
latest selection. Paste reads the desktop clipboard asynchronously and never
executes if the request is stale, its tab or folder changed, or the two-second
UI deadline passed. A completed but unsuccessful desktop lookup may use the
internal clipboard; a timeout never silently replays an old cut list. Desktop
clipboard payloads are limited to 4 MiB, and only one external read worker
may be active at once.

The address bar and CLI use the same local/remote location parser. A valid
`file://` URI resolves to a native path. SMB/SFTP/WebDAV/MTP addresses launch
read-only GIO browsing tabs; an unsupported scheme fails without navigating or
mutating the current local tab. The desktop launcher accepts URIs via `%U`.
URI credentials are rejected rather than retained in status messages or tab
history. The explicitly authorized remote mutations are single-file WebDAV
and restricted OpenSSH-SFTP uploads, independent of read-only GIO browsing.
WebDAV copies a no-follow local source to a private
snapshot, probes `If-None-Match: *` and `MOVE Overwrite: F` (including remote
two-client collisions), compares full staged bytes and requests the server
to publish without overwrite. SFTP requires configured SSH key/agent login,
trusted known_hosts and safe ASCII names/paths. It probes four server-side
hardlink collisions, stages in an exclusive `0700` directory, reads full
staged bytes back and publishes only via `hardlink@openssh.com`.
Unsupported SFTP servers, SMB and MTP remain write-disabled; no generic GIO
fallback, remote rename or delete is allowed.
Unsuccessful uploads can leave private server-side stages for manual review.
Before the **first safety-probe PUT or SFTP mkdir**, the uploader durably saves the random
probe prefix to private, credential-free evidence. Only after all four probe
rounds and their server-side cleanup finish may that record be retired.
Interrupted or refused probes remain in recovery with copyable prefixes and
round 0–3 object-name guidance (SFTP probes use private directories with
`a`, `b` and `target-0` through `target-3`); dismissing a record requires confirmation
and removes local evidence only. Previously orphaned probes from older
versions cannot be reconstructed. Before the first **actual upload PUT or SFTP mkdir**, the
uploader durably publishes a private, bounded,
credential-free evidence journal of the random stage URI and final destination.
If that fails, no staging PUT or SFTP mkdir occurs. A successful HTTP 201 MOVE
or confirmed SFTP publication and private-stage cleanup retires the
evidence. Crash/timeout records appear in a background-loaded recovery review
with copyable URIs and an optional two-step confirmation that discards **only
the local record** after separate server-side verification. No automatic
server queries, retries or DELETE requests run during recovery.
Remote preview reads only a bounded first-32-KiB prefix on a cancellable worker;
larger text is explicitly marked incomplete, and binary/control-bearing bytes
use the shared escaped 256-byte hex preview. The preview does not imply a
complete or verified remote download and never authorizes mutation.

Copy and Move failures identify the failing source's position in the batch and
the number of earlier completed items. The error is not a transaction rollback:
the failed item may have a published destination needing manual inspection,
particularly after an interrupted cross-volume move.

Paste with options offers Keep both, Skip existing without verification,
Require no conflicts, and Skip only matching contents. That last policy uses
bounded read-only no-follow comparison of occupied destinations (including
nested directories and symlink targets), aborting the entire batch when data
differs, cannot be inspected, or exceeds the budget. Equal-content skips never
delete cut sources. The worker reruns preflight after the asynchronous preview;
external writers can still change a path afterward, so no comparison is a
source-cleanup guarantee or a safe replace/overwrite decision.

Multi-item Trash, permanent Delete, Link and Batch Rename follow the same
partial-result reporting rule. Batch rename accepts 2–256 selected entries,
builds names using prefix, suffix, optional literal find/replace and optional
numbering, and displays the complete mapping before confirmation. Prefix,
suffix and numbering preserve raw non-UTF-8 bytes; text replacement explicitly
requires UTF-8 filenames. Permanent Delete must report missing items rather
than silently succeed, unlike idempotent internal staging cleanup. The
worker verifies all source identities and destination vacancies before its
first mutation; individual renames use atomic no-overwrite semantics on Linux.
Concurrent writers can still cause a partial batch and earlier renames remain.

Symbolic-link batches reserve unique destination names before any creation,
use atomic create-if-absent behavior, and report every requested source and
actual link target in Transfers. A concurrent destination collision stops
the batch and preserves the other writer's entry. Already created links are
not silently rolled back; cancelled/unstarted entries remain explicit.

## Ownership and recovery

- **File list/table:** `Explorer::file_listing`, shared for all themes; virtualized rows.
- **Search and clear:** `Explorer::navigation_bar`/`rebuild_visible`.
  The optional Contents mode uses the shared cancellable local worker, with
  visible partial-result notices and no persistent index. The independent
  Subfolders toggle selects current-folder or recursive search. Contents
  never reads special files, follows symbolic links or crosses filesystem
  boundaries; each search is constrained by file-size, byte, count and time
  limits. A filtered-out selection never remains eligible for mutation.
  The theme-independent Filters menu also supports an opt-in filename glob
  and numeric size/age bounds; invalid combinations fail closed and show
  a warning. A single Clear action removes both presets and custom bounds.
  Saved local searches require an explicit action, are capped at 16, and
  restore only query, folder and search scope when opened. No saved search
  triggers background disk or network IO during startup; preferences are
  written via private, atomically replaced files on Unix.
  **Duplicate candidates:** the shared Actions menu starts a cancellable,
  read-only local size/SHA-256 scan, showing groups and partial results in a
  non-destructive viewer. No digest ever authorizes deletion or overwriting;
  changed files must be independently reverified by the user.
- **Dialog/confirmation:** `Explorer::dialogs`; no system-wide native dialog side effects.
- **Feedback:** `Explorer::status_bar` when enabled, otherwise the shared
  `Explorer::transfer_strip` displays the latest status with a full-message
  tooltip. Errors must remain visible even in themes without a bottom bar.
- **Unfinished cross-device moves:** `journal::scan_incomplete` runs on a
  background thread; `Explorer::recovery_banner` remains visible regardless
  of theme/status-bar settings. Journal states do not authorize automatic
  deletion, resume or overwriting; verified unpublished file/link publication
  requires a staged-content comparison, fresh checks and two-step confirmation.
  Show source and destination saved-inode evidence independently, with clear missing,
  changed and replaced states. Source/destination browse buttons navigate to
  their parent folders; a verified directory staging path may be opened
  directly for read-only inspection, but never through a symlink. The staged
  payload comparison cannot unlock the published-copy journal-resolution action. An optional
  separate Copy is only offered for a visible source with matching recorded
  identity, opens normal asynchronous Paste options with Keep both and an
  explicit second confirmation, and never modifies journal evidence or
  performs destructive move cleanup.
- **Operations:** `filesystem::perform`; worker response includes actual completion result.
- **Properties:** `Explorer::open_properties` schedules one background metadata
  reader at a time; a later request replaces the pending one. The shared
  Properties dialog exposes loading/error states, ignores stale results and
  offers permission editing only after metadata has loaded for that entry.
  Closing the dialog never waits for an offline filesystem.
- **Transfer queue:** `operation_queue::OperationQueue` owns order, IDs and
  the 32-job bound; `Explorer::start_next_operation` alone dispatches workers.
  `Explorer::transfer_strip` and `transfer_dialog` are shared by all shells.
  Pending copies are volatile and never replay automatically after restart.
  The dialog keeps 64 recent session outcomes (completed, cancelled, failed)
  without claiming that earlier successful items in a failed batch rolled back.
- **Theme:** `Theme::apply` and runtime TOML files; no theme-specific copy/move behavior.
  RiceBowl may set `RICEBOWL_RICE` or the fallback `RICEBOWL_THEME` for one
  launch; only known built-in names/aliases are accepted. An explicit CLI
  `--theme` wins, and neither the CLI nor the environment selection rewrites
  the saved user theme when other preferences are stored. Explicit selection
  from RiceField's own UI does persist normally.
  The shared **Open With** dialog uses the desktop MIME registry only for a
  single local file; desktop IDs and MIME types are validated before a
  shell-free GIO launch. Slow/unavailable backends show a cancellable loading
  or error state, and no file content is copied to an external process by the
  lookup itself. Local file context menus and the preview pane invoke the
  same chooser, including recommended MIME handlers.
  External theme files are observed at the shared idle cadence, reloaded only
  after a metadata change, and limited to regular 256 KiB inputs. A broken
  edit never replaces the last working theme or touches open tabs/transfers;
  selecting a built-in profile stops watching the external file.
- **Shell composition:** `Theme::shell_kind`, `top_components` and
  `bottom_components` place maintained controls in distinct regions. A chrome
  component may appear only once; path navigation must remain accessible.
  Shell changes never replace state or the semantics/safety of mutations.
- **Sidebar composition:** `Theme::sidebar_sections`, `sidebar_position`,
  `sidebar_width` and `sidebar_section_mode` provide per-rice layout. XP-style
  collapsible groups retain their open/closed state independently. The optional `tasks` section uses
  the same New Folder/Copy/Cut/Paste/Rename/Trash/Properties handlers as the
  top menus. Task availability follows the current selection.
- **Preview placement:** `Theme::preview_position` and `preview_width` govern
  only on-demand preview geometry, not which files are read or decoded.
- **Settings:** XDG config TOML, transactional replacement, recoverable defaults for invalid content.

Accessibility target is WCAG 2.2 AA. Explicit known gap in v0.1: egui's native screen-reader semantics and modal focus trapping need end-to-end accessibility testing before accessibility compliance can be claimed. Future releases must address it rather than stating compliance prematurely.

Changing the active shell must preserve tabs, selection, bookmarks and any
running operation. Unsupported theme schema versions or unknown UI components
must fail validation and fall back safely. Every visible quick action performs
the same underlying operation across all shell identities.

## Verification

Unit tests for theme parsing, self-copy rejection, collision-free copying, protected rename. Cargo formatting, Clippy and tests run in CI. Manual checks on Mint X11 and Wayland remain required before a stable release, including keyboard, narrow window, inaccessible folder, long filenames, large directories, theme switching and Trash recovery.
