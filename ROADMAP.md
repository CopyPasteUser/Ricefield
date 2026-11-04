# RiceExplorer roadmap to 1.0

**Goal:** a reliable, customizable Linux file manager that can replace
Thunar for supported workflows. Keep Thunar installed until real Linux Mint
acceptance passes. Prioritize data integrity, functionality, stability,
performance, Linux integration, then visual polish.

## Baseline: v0.2.0 (code-level inventory)

Implemented: Rust/egui application, seven bundled theme profiles, tabs,
history, bookmarks, list/icon views, background local scans, bounded local
search, clipboard helpers, local create/rename/copy/move/trash/delete/link,
file properties, basic permission editing and image/text previews. Cross-device
moves copy, compare bytes, then remove the source. A Linux CI workflow exists.

Partial: mounted path discovery, desktop clipboard, cancellation, preview
coverage, accessible controls and Xvfb startup tests. Not implemented:
full operations queue/recovery, native GIO storage, mount actions, network
connections, archives, undo, split view, FileManager1 and theme editor.

At baseline commit `214e585`, `main` and
`feature/functional-filemanager-v02` are identical. This inventory is
**not** a claim of successful hardware, GUI or performance tests.

## Priority and acceptance criteria

| Priority | Work | Exit condition |
| --- | --- | --- |
| P0 | Crash-safe writes/moves | No incomplete destination appears complete; recoverable partials and preserved source on interruption |
| P0 | Destructive actions | Clear confirmation, cancellability limits and per-file partial outcomes; no silent loss |
| P0 | Failure-injection tests | Disposable fixtures cover cancellation, collisions, cross-device moves, disk full, permissions, symlinks and source changes |
| P0 | Responsive UI | Clipboard, properties and filesystem/network operations cannot indefinitely block the render thread |
| P0 | Operation recovery | Journal and restart tests distinguish pending, committed and failed transfers |
| P1 | Transfer UX | Queue, bounded concurrency, progress/rate/ETA, conflict dialog, undo when safe, batch rename preview |
| P1 | Storage and network | GIO/GVfs SMB/SFTP/WebDAV, system NFS, UDisks2/Polkit mount/eject, secrets, reconnect and offline errors |
| P1 | Archives | ZIP, TAR variants, 7z and optional RAR with traversal, symlink and bomb protection |
| P1 | Explorer functions | Split view, session restore, drag-out, folder tree, context actions and robust keyboard interaction |
| P1 | Preview and search | Bounded thumbnails, PDF/media/code/archive preview, metadata, filters and cancellable search |
| P1 | Linux integration | MIME/Open With, real URI opening, FileManager1, portals, X11/Wayland and i18n |
| P1 | Composable rice architecture | Versioned TOML shell composition, shared actions, independently replaceable UI panels; no core rewrite |
| P2 | RiceBowl polish | Seven distinctive shells, live theme IPC, editor, custom Rust component registration and accessibility refinement |
| P2 | Organization extras | Saved searches, duplicate detection, optional index/tags, ACLs and extended attributes |

## Version gates

Each release requires implementation, meaningful tests and documentation.
Untested hardware and GUI behavior is reported as **outstanding**, not done.
`0.3.0-alpha.1` is a development preview for the completed 0.3 safety
increments, **not** a passed final v0.3 gate. Its remaining unchecked items
must not be implied complete by the package version.

### 0.2 — Stability and existing behavior

- [x] Local file manager and seven bundled profiles.
- [x] Initial CI workflow, worker operations and progress reporting.
- [x] Fix name-collision byte preservation, dialog target snapshots,
  launcher path handling and seven-theme smoke coverage.
- [x] Establish a versioned shell schema, component selection and ordering
  without breaking legacy themes or file operations.
- [ ] Test remaining baseline safety and UI defects.
- [ ] Complete real Linux Mint X11 keyboard/mouse/visual acceptance.

**Gate:** no silent overwrite or known data-loss bug in the supported basic
workflow. Regression tests run in CI; Thunar remains the fallback.

### 0.3 — Reliable file operations

- [ ] Verify initial hidden staging and atomic no-overwrite publication for
  local copies and cross-device moves in CI and on Linux Mint.
- [ ] Eliminate the remaining cross-device source mutation race and preserve
  timestamp, owner, ACL, xattr and hardlink metadata before production use.
- [x] Refuse staged publication if a source's identity, size, modification
  time, inode change time or mode changes during the copy; open regular
  sources without following
  a last-moment symlink replacement. Preserve regular-file and directory
  access/modification times in staged copies. Still require race-free source
  cleanup, symlink timestamps, ACL/xattr/ownership/hardlink preservation.
- [x] Retain regular-file hardlink relationships when multiple names for the
  same inode occur inside a single directory tree being copied. Cross-item
  and out-of-tree links, extended metadata and ACL preservation remain open.
- [x] Snapshot every source entry before cross-device Move staging and
  revalidate the entire manifest before cleanup and each entry before unlink.
  Refuse a same-content replacement inode; handle ctime/link-count changes
  caused by our own earlier hardlink removals. The final check/unlink is
  still not atomic under a concurrent writer.
- [x] Preserve readable Linux `user.*` xattrs in staged files, directories
  and symlinks; refuse publication when the target cannot accept a copied
  attribute. System ACLs, privileged namespaces and ownership remain open.
- [x] Introduce a bounded, in-memory FIFO for up to 32 waiting Copy jobs,
  stable operation IDs, visible progress in every rice, and cancellation of
  waiting jobs without filesystem side effects. Pause the queue after an
  operation error/cancellation until the user explicitly resumes it. Keep a
  bounded 64-entry session-only result history with success/failure/cancel.
- [ ] Extend the queue to Move and other operations only after stale-source
  preflight and safe confirmation are implemented; add persistent history
  and failure recovery. Per-item results and elapsed time/rate/ETA are tracked
  below as complete; in-memory history is not durable.
- [x] Show elapsed wall-clock time, average transferred-byte throughput and a
  conservative approximate ETA on active transfers across all rice layouts;
  show final elapsed time in session results. Hide estimates before enough
  progress has been measured, and test rate/ETA formatting.
- [x] Add structured Copy/Move results for every requested source, including
  completed/renamed targets, explicit skips, failures, cancellations and
  unstarted items. Show a bounded-height, virtualized detail list in Transfers
  for all rices. Preflight failures identify the offending input without
  making changes; preserve skipped cut sources for retry. Results are only
  retained in the current session, with no automatic rollback or replay.
- [x] Add a side-effect-free, asynchronous Paste preflight with per-item
  destinations, reserved targets for repeated basenames, automatic rename/skip
  counts and a third *Require no conflicts* policy. Repeat the complete
  preflight on the worker before mutations; reject duplicate/overlapping
  sources and destinations inside selected folders. Preserve only unfinished
  cut items after a partially completed **failed** Move, not just success.
- [x] Validate all selected Trash/Delete paths before any mutation, rejecting
  missing, duplicate or ancestor/descendant selections. Recheck each source's
  identity before its removal, report per-item outcomes and item-count progress;
  show selected paths before confirmation even in large batches.
  A later runtime failure remains a partial operation; directory traversal is
  not a race-free transaction. Add regression tests for missing later inputs,
  overlaps and source changes during a batch.
- [x] Report the index, source and count of earlier committed items when a
  multi-item Copy or Move stops; extend this report to Trash, Delete, Link and
  Batch Rename. This is an error diagnostic, not yet a full
  structured per-item result or rollback system.
- [ ] Finish advanced conflict choices (verified replace/compare, rename
  manually, per-item overrides) and recoverable user-facing error decisions.
  Current whole-batch preflight, Keep both, Skip existing and Abort on conflict
  are safe but do not reserve destination names against external writers.
- [x] Offer a non-destructive per-transfer Keep both / Skip existing choice
  through the universal Actions menu; apply it to multi-item Copy and Move
  on the worker, preserve skipped cut sources, and report counts. Test mixed
  batches and dangling symlink destinations. Full-batch read-only preflight
  now catches known conflicts and unsafe source layouts; it is **not** a
  replace/compare/rename recovery workflow. Racing writes may still stop a
  transfer safely.
- [x] Stage regular output and copy directories privately, sync staged files,
  subdirectories and the staging parent, then publish by atomic no-overwrite
  rename and sync the destination parent. If a directory sync fails after
  publication, report the uncertain state and preserve the original source.
  Verify failure behavior with injected staging and publication sync errors.
  Metadata completeness, source cleanup durability, FUSE/remote behavior and
  power-loss fault injection remain release blockers.
- [x] Add a durable cross-device move state journal with a read-only scan on
  startup, a visible warning and manual inspection of unfinished transfers.
- [x] Inspect source, destination and staging path presence asynchronously
  during recovery scans, without following symlinks or writing to any path.
  Distinguish missing, present and inaccessible entries; show conservative
  per-record guidance. Coalesce concurrent rescan requests. Presence alone
  cannot justify source cleanup or dismissing a journal.
- [x] Offer opt-in background, cancellable, bounded read-only comparisons of
  recorded source/destination contents (including nested bytes and symlink
  targets) with clear match/mismatch/inconclusive results. Check snapshots
  before/after reads; do not interpret matches as permission to delete.
- [ ] Add durable provenance evidence, safe restart resume/rollback and
  race-free source cleanup; a matching read-only scan is insufficient.
- [x] Isolate test journals and GUI smoke XDG state from the developer's real
  state, including tests that intentionally leave unfinished transfers.
- [ ] Complete restart recovery with verified resume/rollback choices, bounded
  journal retention, interrupted-publication/power-loss fault injection,
  real mount/USB tests and source deletion safeguards. No automatic deletion
  is permitted based solely on a saved journal entry.
- [ ] Cancel, safe pause/resume where possible, undo for reversible actions.
- [x] Implement bounded 2–256 item batch rename with a live prefix/suffix
  preview, before-start collision preflight and no-overwrite renames. Preserve
  non-UTF-8 filename bytes; test collisions and mid-batch partial failures.
- [x] Add zero-padded numbering and literal search/replace to the complete
  preview without new dependencies. Refuse search/replace of non-UTF-8 names.
- [ ] Extend batch rename to regex, safe rename cycles, rollback/undo where
  possible; test hardlinks and complete metadata behavior.

**Gate:** fault-injected copy/move/crash scenarios do not silently lose user
data; transfers never block the UI.

### 0.4 — Volumes and network files

- [ ] Model local paths and remote URIs without discarding local operations.
- [ ] Integrate GIO/GVfs with a correctly driven GLib main context.
- [ ] Discover drives/volumes/mounts and their capabilities dynamically.
- [ ] Mount/unmount/eject/unlock through OS APIs and Polkit.
- [ ] Connect SMB/SFTP/WebDAV, browse system NFS and supported MTP devices.
- [ ] Connect dialog, secure credential handling, favorites and reconnect.

**Gate:** offline shares, disconnections, bad credentials and removed USB
devices produce clear errors without frozen UI or silent data loss.

### 0.5 — Archive management

- [ ] Compress/extract ZIP, tar.gz, tar.xz, tar.zst and 7z; optional RAR.
- [ ] Entry browsing, selected extraction, integrity check, progress/cancel.
- [ ] Password prompts where supported, no plaintext password persistence.
- [ ] Reject traversal, symlink escape, archive bombs and unsafe overwrites.

**Gate:** hostile archives cannot write outside the selected destination.

### 0.6 — Everyday navigation and quick actions

- [x] Extend declarative shell composition to top/bottom chrome, XP-style
  task modules in the sidebar and left/right preview placement; keep old
  TOML theme files compatible and enforce safe structural validation.
- [x] Support skin-selectable collapsible versus flat sidebar modules; XP
  task groups can fold without changing file operations or legacy skins.
- [x] Remember up to 32 absolute UTF-8 tab locations and the active tab across
  app restarts, with CLI path override and an opt-out that clears stored paths.
  Restore without statting possibly offline mounts on the render thread.
- [ ] Split view, tab duplicate/reorder, deeper history/session restore and
  multiple windows.
- [ ] Extract shared navigation, file list, toolbar, sidebar and dialog
  primitives so new shell renderers consume a common command dispatcher.
- [ ] Real drag in/out, context menus, toolbar actions and keyboard shortcuts.
- [ ] Folder tree, enhanced details/grid views, natural sorting and filters.
- [ ] Responsive accessible dialogs, robust error/empty/loading states.

**Gate:** important local workflows work by mouse and keyboard in all themes.

### 0.7 — Previews and metadata

- [x] Bound image preview input to 32 MiB on the opened file descriptor,
  check the target of symlinks, reject special files and retain the 16 MP
  pixel limit; add preview safety regressions.
- [ ] Bounded thumbnail cache and lazy priorities for visible entries.
- [ ] Quick Look and safe image/PDF/text/code/audio/video/archive preview.
- [ ] Metadata, on-demand folder size and hashes, ACL/xattr support.

**Gate:** malformed or huge files cannot crash/freeze the GUI.

### 0.8 — Search and organization

- [x] Cancel obsolete local folder scans and recursive filename searches on
  new requests, tab closure and query changes; reject queued stale results.
- [ ] Filename/content searches, type/date/size/regex filters.
- [ ] Bounded cancellable network search, optional index (not mandatory).
- [ ] Saved searches, duplicate finder and batch action previews.

**Gate:** 100k-entry folders and recursive searches remain responsive.

### 0.9 — RiceBowl and Linux desktop integration

- [ ] MIME/Open With, FileManager1, portals, URI and clipboard interoperability.
- [ ] Live RiceBowl theme sync, TOML editor/import/export and safe fallback.
- [ ] Seven distinct shells and matching icons, controls and layouts.
- [ ] Confirm Windows 98, Windows 7 and macOS render three materially
  different component structures; support custom Rust components via a
  compile-time registry without an unsafe runtime plugin loader.
- [ ] German/English localization, HiDPI, multi-monitor, accessibility review.

**Gate:** complete Mint X11 and Wayland functional/visual matrix passes.

### 1.0 — Production release

- [ ] Format, Clippy, unit/integration/UI/security tests pass in CI.
- [ ] Measure startup, idle RAM/CPU, 100k entries, HDD and large transfers.
- [ ] Test 4 GB hardware, network disconnect and USB removal in a safe lab.
- [ ] Verify install, update, uninstall, rollback and recoverability.
- [ ] Ship current README, changelog, actual screenshots and release assets.
- [ ] Resolve critical bugs and obtain real Linux Mint user sign-off.
- [ ] Verify that a new rice can be assembled from configured components
  without changing the file-management core or interrupting transfers.

**Definition of done:** dependable daily file management for explicitly
supported devices and protocols, recoverable failure modes, accurate
documentation and measured performance. Untested features stay open.
