# RiceExplorer UX contract

## Scope

Linux desktop file management for RiceBowl users. v0.1 interface English; runtime GUI built in egui. Visual language is owned by [DESIGN.md](DESIGN.md) and `themes/*.toml`. The filesystem module is the canonical owner for file mutation.

## Navigation and selection

- Tabs preserve their own path, back/forward history, search query, selection and pending scan generation.
- Entering a new location clears local search and selection, but not bookmarks or preferences.
- Sorting always groups directories before files. Filters operate only on the current folder; no unrequested recursive search/indexing.
- Shift selects a contiguous visible range; Ctrl toggles one item. Navigation history is independent of filesystem mutation history.
- Keyboard shortcuts do not copy/cut/paste files while a text field has focus.

## Mutations

| Action | Pending | Success | Failure |
| --- | --- | --- | --- |
| New file/folder | Background worker | Refresh all tabs; status message | Keep existing folder; error in status |
| Rename | Background worker | Refresh tabs; status message | Existing target remains; error in status |
| Copy | Background worker | Unique new destination; status | Partial new output cleaned when possible; source intact |
| Move | Background worker | Rename within volume; status | Original remains for failed individual move; report error |
| Trash | Explicit confirmation, then worker | Platform Trash; status | Report error; refresh tabs to show partial state |

Only one mutation worker runs at a time; folder reads may proceed concurrently. Copy requests arriving during another mutation may enter a bounded 32-item FIFO queue with stable IDs. Queued jobs run after a successful active worker; after failure or cancellation they pause until explicit Resume, so further transfers do not cascade into a known failure. Each waiting job can be removed before execution without touching files. Other mutations are not delayed automatically, to avoid executing stale destructive requests. There is no fake optimistic success. Copy/cut uses an application-local file clipboard for v0.1, not the desktop MIME clipboard. The Trash confirmation is deliberately conservative even though desktop Trash is recoverable.

Copy and Move failures identify the failing source's position in the batch and
the number of earlier completed items. The error is not a transaction rollback:
the failed item may have a published destination needing manual inspection,
particularly after an interrupted cross-volume move.

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
  states do not authorize automatic deletion, resume or overwriting.
- **Operations:** `filesystem::perform`; worker response includes actual completion result.
- **Transfer queue:** `operation_queue::OperationQueue` owns order, IDs and
  the 32-job bound; `Explorer::start_next_operation` alone dispatches workers.
  `Explorer::transfer_strip` and `transfer_dialog` are shared by all shells.
  Pending copies are volatile and never replay automatically after restart.
  The dialog keeps 64 recent session outcomes (completed, cancelled, failed)
  without claiming that earlier successful items in a failed batch rolled back.
- **Theme:** `Theme::apply` and runtime TOML files; no theme-specific copy/move behavior.
- **Shell composition:** `Theme::shell_kind` and `Theme::top_components` select
  existing controls and their order. A shell may change the interface, never
  the semantics or safety checks of copy/move/Trash.
- **Sidebar composition:** `Theme::sidebar_sections`, `sidebar_position` and
  `sidebar_width` provide per-rice layout without altering bookmarks, mount
  discovery or navigation semantics.
- **Settings:** XDG config TOML, transactional replacement, recoverable defaults for invalid content.

Accessibility target is WCAG 2.2 AA. Explicit known gap in v0.1: egui's native screen-reader semantics and modal focus trapping need end-to-end accessibility testing before accessibility compliance can be claimed. Future releases must address it rather than stating compliance prematurely.

Changing the active shell must preserve tabs, selection, bookmarks and any
running operation. Unsupported theme schema versions or unknown UI components
must fail validation and fall back safely. Every visible quick action performs
the same underlying operation across all shell identities.

## Verification

Unit tests for theme parsing, self-copy rejection, collision-free copying, protected rename. Cargo formatting, Clippy and tests run in CI. Manual checks on Mint X11 and Wayland remain required before a stable release, including keyboard, narrow window, inaccessible folder, long filenames, large directories, theme switching and Trash recovery.
