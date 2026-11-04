# RiceExplorer UX contract

## Scope

Linux desktop file management for RiceBowl users. v0.1 interface English; runtime GUI built in egui. Visual language is owned by [DESIGN.md](DESIGN.md) and `themes/*.toml`. The filesystem module is the canonical owner for file mutation.

## Navigation and selection

- Tabs preserve their own path, back/forward history, search query, selection and pending scan generation.
- The session snapshot keeps at most 32 absolute, UTF-8 tab paths and one
  active index. An explicit launch path overrides restoration, and opting out
  clears the stored paths. Unavailable mounts are handled by asynchronous
  folder scans; never synchronously probe them during session hydration.
  Search/history/selection and filesystem mutations are never replayed.
- Each tab owns a cooperative scan cancellation token. Navigation, refresh,
  closure and changes to recursive search queries invalidate superseded
  workers; cancelled/queued stale results must never replace current entries.
- Entering a new location clears local search and selection, but not bookmarks or preferences.
- Sorting always groups directories before files. Filters operate only on the current folder; no unrequested recursive search/indexing.
- Shift selects a contiguous visible range; Ctrl toggles one item. Navigation history is independent of filesystem mutation history.
- Keyboard shortcuts do not copy/cut/paste files while a text field has focus.

## Mutations

| Action | Pending | Success | Failure |
| --- | --- | --- | --- |
| New file/folder | Background worker | Refresh all tabs; status message | Keep existing folder; error in status |
| Rename | Background worker | Refresh tabs; status message; session-only Undo in Transfers if a post-rename inode snapshot is captured | Existing target remains; error in status |
| Batch rename | Snapshot selection, preview all candidate names, preflight targets on worker | Rename each item without overwriting | Stop with failed item and completed count; no silent rollback claim |
| Copy | Background worker | Unique new destination; status | Partial new output cleaned when possible; source intact |
| Move | Background worker | Rename within volume; status | Original remains for failed individual move; report error |
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

Copy/cut immediately updates the internal file clipboard and synchronizes
the desktop clipboard in a worker. Outstanding desktop writes coalesce to the
latest selection. Paste reads the desktop clipboard asynchronously and never
executes if the request is stale, its tab or folder changed, or the two-second
UI deadline passed. A completed but unsuccessful desktop lookup may use the
internal clipboard; a timeout never silently replays an old cut list. Desktop
clipboard payloads are limited to 4 MiB, and only one external read worker
may be active at once.

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
- **Dialog/confirmation:** `Explorer::dialogs`; no system-wide native dialog side effects.
- **Feedback:** `Explorer::status_bar` when enabled, otherwise the shared
  `Explorer::transfer_strip` displays the latest status with a full-message
  tooltip. Errors must remain visible even in themes without a bottom bar.
- **Unfinished cross-device moves:** `journal::scan_incomplete` runs on a
  background thread; `Explorer::recovery_banner` remains visible regardless
  of theme/status-bar settings. The review window is inspection-only: journal
  states do not authorize automatic deletion, resume or overwriting. Show source
  and destination saved-inode evidence independently, with clear missing,
  changed and replaced states. Browse buttons navigate to the recorded paths'
  parent folders without following the recorded entry as a symlink. An optional
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
