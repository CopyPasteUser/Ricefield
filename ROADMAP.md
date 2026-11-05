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
`0.3.0-alpha.4` is a development preview for the completed 0.3 safety
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

- [x] Produce byte-reproducible Linux x86_64 archives with checksum manifests,
  unprivileged installer, isolated install/uninstall smoke and CI tag-gated
  release automation. The release workflow is **not** evidence of complete
  safety, recovery or physical Linux Mint acceptance.
- [ ] Verify initial hidden staging and atomic no-overwrite publication for
  local copies and cross-device moves in CI and on Linux Mint.
- [x] Remove automatic source deletion from cross-device Move: publish a
  verified Copy and keep the original for manual cleanup. This sidesteps the
  unfixable verify-then-unlink race for new cross-volume transfers, but does
  not prove metadata completeness or production readiness.
- [ ] Finish timestamp, ownership, ACL/xattr, privileged metadata and
  hardlink interoperability across real target filesystems before production use.
- [x] Refuse staged publication if a source's identity, size, modification
  time, inode change time or mode changes during the copy; open regular
  sources without following
  a last-moment symlink replacement. Preserve regular-file and directory
  access/modification times in staged copies. Still require race-free source
  cleanup, ACL/privileged-xattr/ownership/hardlink preservation.
- [x] On Linux preserve symbolic-link access and modification times without
  following links, including dangling links. Fail staged publication if link
  metadata cannot be preserved. Verification now opens regular source and
  destination files with no-follow/nonblocking flags and rechecks inode,
  ctime, size and mode through their descriptors and paths. Also prevent
  source files swapped for FIFOs from hanging a copy worker. This narrows
  replacement races but does not make recursive cleanup atomic.
- [x] Retain regular-file hardlink relationships when multiple names for the
  same inode occur inside a single directory tree being copied. Cross-item
  and out-of-tree links remain outside the scope of Copy. Cross-device Move
  now refuses out-of-tree regular-file links and hardlinked symlinks before
  staging rather than silently severing them. Extended metadata and ACL
  correctness are still subject to strict target equivalence checks.
  Reject a staged hardlink reuse if its source inode was changed since the
  first copy, instead of silently reusing stale bytes.
- [x] Snapshot every source entry before cross-device Move staging and
  revalidate the entire manifest before cleanup and each entry before unlink.
  Refuse a same-content replacement inode; handle ctime/link-count changes
  caused by our own earlier hardlink removals. The final check/unlink is
  still not atomic under a concurrent writer. **Historical implementation:**
  new cross-device transfers never invoke that cleanup path.
- [x] Preserve readable Linux `user.*` xattrs in staged files, directories
  and symlinks; refuse publication when the target cannot accept a copied
  attribute. POSIX ACLs are now handled separately; privileged xattr
  namespaces are still not migrated.
- [x] Preserve Linux POSIX access/default ACLs on staged files/directories,
  including removal of unintended ACL inheritance from staging parents.
  Attempt to preserve uid/gid without following symlinks when permitted;
  allow an ordinary Copy to continue with its new owner if not permitted,
  but reject an ownership mismatch for a verified cross-device Move.
  Privileged metadata, external hardlink identity and complete ownership
  support across filesystems remain open. Cross-device source cleanup is now
  intentionally manual rather than an unsafe automatic operation.
- [x] Before publishing a verified cross-device Move or removing its source,
  require equivalent Unix mode, owner/group and nanosecond modification time
  for each copied entry. On Linux also compare **all** visible xattr names
  and values without following symlinks, including ACL/security namespaces;
  fail closed when unreadable or different. This blocks lossy moves but does
  **not** implement ownership/ACL migration or guarantee that another process
  cannot change source metadata afterward. The final unlink was removed.
- [x] Introduce a bounded, in-memory FIFO for up to 32 waiting Copy jobs,
  stable operation IDs, visible progress in every rice, and cancellation of
  waiting jobs without filesystem side effects. Pause the queue after an
  operation error/cancellation until the user explicitly resumes it. Keep a
  bounded 64-entry session-only result history with success/failure/cancel.
- [x] Offer manual Pause / Resume for the waiting Copy queue in the shared
  Transfers dialog. Running operations are not suspended mid-file, and this
  session-only control does not imply restart replay or safe Undo.
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
- [x] Move external file clipboard copy/cut/paste calls off the GUI thread,
  coalesce outstanding writes and limit reads to 4 MiB. An unresponsive paste
  provider gets a two-second UI deadline and cannot issue a late mutation;
  a tab/folder change also cancels the pending paste. Properties and remaining
  filesystem/network UI calls still require a separate responsiveness audit.
- [x] Load file properties and Unix permission metadata in a bounded
  background worker, not on each render frame. Keep only the latest requested
  dialog when a slow mount stalls an earlier query; reject stale results and
  disable permission edits until the current snapshot is available. More
  render-thread filesystem call sites still require review.
- [x] Fault-inject a destination created after staging but before atomic
  publication and a source modified after staging verification. Confirm no
  overwrite, no source cleanup, correct historical journal evidence and no staging debris
  in both cases. Physical mount/USB publication tests remain outstanding.
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
- [x] Provide structured per-item Link outcomes, read-only unique-destination
  preflight and item-based progress. Test same-basename sources, a competing
  writer after the first completed link, and cancellation of later items.
  Completed links are not automatically rolled back or undone.
- [ ] Finish advanced conflict choices (verified replace/compare, rename
  manually, per-item overrides) and recoverable user-facing error decisions.
  The additional content-verified **Skip only matching contents** policy now
  compares existing files, nested folders and symlink targets on a bounded
  background preflight, aborts if contents differ or cannot be checked, and
  leaves skipped Move sources intact. It is deliberately **not** verified
  replacement, manual per-item rename/override, atomic protection against
  external writers or Undo. The remaining advanced choices are still open.
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
  Metadata completeness, FUSE/remote behavior and
  power-loss fault injection remain release blockers.
- [x] Add a durable cross-device move state journal with a read-only scan on
  startup, a visible warning and manual inspection of unfinished transfers.
  Its record reader now pins regular inodes without following replacement
  symlinks or blocking on substituted FIFOs, and checks for changes after
  bounded reads. Full resume/rollback remains open.
- [x] Inspect source, destination and staging path presence asynchronously
  during recovery scans, without following symlinks or writing to any path.
  Distinguish missing, present and inaccessible entries; show conservative
  per-record guidance. Show source and destination identity status separately
  and allow direct navigation to their containing folders and the staging
  parent for manual triage, without following the saved entry as a symlink.
  Offer a separate, preflighted non-destructive Copy shortcut only for a source
  whose recorded identity still matches; require explicit Start and retain the
  unresolved journal. This is **not** safe restart Move replay or rollback.
  Coalesce concurrent rescan requests. Presence alone cannot justify source
  cleanup or dismissing a journal.
- [x] Offer opt-in background, cancellable, bounded read-only comparisons of
  recorded source/destination contents (including nested bytes and symlink
  targets) with clear match/mismatch/inconclusive results. Check snapshots
  before/after reads; do not interpret matches as permission to delete.
- [x] Persist Unix source inode and metadata evidence before staging and
  published-destination identity after the durable publication barrier.
  Journal v3 additionally persists the staged payload inode identity after
  verification and refuses the Published transition unless the destination
  still names that same inode. After restart the read-only review shows this
  relationship even after publication with an incomplete StagedVerified record;
  v1/v2 records retain read-only legacy compatibility.
  Recovery inspection distinguishes changed and replaced recorded objects;
  v1 records remain inspectable without pretending that evidence exists.
  v2 records with missing evidence are rejected. Refuse new move journals at
  the 4,096-record scan cap without deleting incomplete recovery evidence.
  Serialize slot reservation across cooperating Unix processes with a
  no-follow directory lock; test simultaneous last-slot claims and a crash
  between publication and the durable Published journal transition.
  These snapshots are advisory and do not prove unchanged contents or permit
  automatic deletion; full provenance, crash replay and retention remain open.
- [x] Reclaim structurally valid `Completed` journal entries only when the
  bounded journal capacity is exhausted, validating with the same no-follow
  reader used for recovery. Keep incomplete, malformed or changing records,
  and sync the journal directory after cleanup. No incomplete record expiry,
  resume, rollback or comprehensive long-term retention policy is implied.
- [ ] Complete durable content provenance and safe restart resume/rollback
  for legacy Move journals; a matching read-only scan is insufficient.
  New cross-volume transfers do not automatically delete the original.
- [x] Isolate test journals and GUI smoke XDG state from the developer's real
  state, including tests that intentionally leave unfinished transfers.
- [ ] Complete restart recovery with verified resume/rollback choices, bounded
  journal retention, interrupted-publication/power-loss fault injection,
  real mount/USB tests and source deletion safeguards. No automatic deletion
  is permitted based solely on a saved journal entry.
- [ ] Cancel, safe pause/resume where possible, undo for reversible actions.
  Individual completed Rename actions now offer a session-only Undo in
  Transfers, guarded by the post-rename source snapshot and no-overwrite
  semantics. Broader Undo (especially partial batches, copies, cross-device
  moves and permanent delete) remains open and cannot rely on stale history.
- [x] Implement bounded 2–256 item batch rename with a live prefix/suffix
  preview, before-start collision preflight and no-overwrite renames. Preserve
  non-UTF-8 filename bytes; test collisions and mid-batch partial failures.
- [x] Add zero-padded numbering and literal search/replace to the complete
  preview without new dependencies. Refuse search/replace of non-UTF-8 names.
- [ ] Extend batch rename to regex, safe rename cycles, rollback/undo where
  possible; test hardlinks and complete metadata behavior.

**Gate:** fault-injected copy/move/crash scenarios do not silently lose user
data; transfers never block the UI. **Still open** for stable 0.3: full
recovery coverage, metadata/mount tests and deferred physical Mint acceptance.

### 0.4 — Volumes and network files

- [x] Introduce a typed Local/Remote URI parser shared by CLI and address bar;
  retain native non-UTF-8 local paths and `file://` decoding. Validate
  SMB/SFTP/DAV/DAVS/MTP URI syntax, reject embedded credentials, and return a
  visible unsupported-backend error without accidentally scanning local paths.
- [ ] Evolve navigation, tab history, entries and worker results to carry
  typed remote locations end-to-end; preserve all local POSIX operations.
  Read-only remote URI tabs now have shared local/remote history and bounded
  worker scans without exposing any remote path to the POSIX file
  engine. Validated remote tabs now persist in the opt-out 32-tab session
  snapshot. Unified entry operations and remote search are still open.
- [ ] Integrate GIO/GVfs with a correctly driven GLib main context.
  `gio` commands currently run in background child processes with their own
  GLib contexts, limits and timeouts. In-process native GIO integration and
  robust cancellation are still open.
- [ ] Discover drives/volumes/mounts and their capabilities dynamically.
  Existing background mountinfo enumeration now labels detected network mounts
  separately in the Volumes sidebar. Device capabilities and unmounted drives
  still require UDisks2/GIO integration.
- [ ] Mount/unmount/eject/unlock through OS APIs and Polkit.
  The Volumes sidebar can now invoke bounded background GIO unmount/eject
  commands after confirmation; drive unlocking and a native Polkit flow are
  still open. No forced unmount is offered.
- [ ] Connect SMB/SFTP/WebDAV, browse system NFS and supported MTP devices.
  Initial non-blocking `gio mount` connection attempts are supported without
  URI credentials. Read-only remote listing works in normal tabs; native remote
  file operations and interactive authentication are not yet available. A
  bounded metadata listing distinguishes traversable directories from files
  that cannot yet be opened or transferred. Small UTF-8 remote files can now
  be inspected through a bounded, cancellable, read-only text preview; binary
  preview, downloading and writes still require the native operation backend.
- [ ] Connect dialog, secure credential handling, favorites and reconnect.
  A basic URI dialog, 32 validated persistent URI favorites and manual
  reconnect are implemented. Credential prompts/keyring integration and
  recovery from mid-operation disconnects remain open.

**Gate:** offline shares, disconnections, bad credentials and removed USB
devices produce clear errors without frozen UI or silent data loss.

**Owner decision (2026-11-04):** Defer keyring/interactive credential work and
all real-device, unplug and disconnection acceptance until a later phase.
Keep remote mutations disabled; do not present these untested integrations as
stable or infer that skipping the tests satisfies the 0.4 release gate.

### 0.5 — Archive management

- [x] Compress/extract ZIP, tar.gz, tar.xz, tar.zst and 7z through runtime
  libarchive. Read/extract uncompressed `.tar` and optionally supported RAR;
  RAR creation is out of scope. All operations are local only.
- [x] Background archive entry browser, selected extraction (including
  selected directories), full-stream integrity check, bounded progress
  updates and cooperative cancellation in a shared archive dialog.
- [x] Masked, session-only password field for reading encrypted ZIP and 7z/RAR
  variants supported by installed libarchive; optional AES-256 encrypted ZIP
  creation. No password persistence or keyring; missing/incorrect credentials
  fail without publishing partially extracted files.
- [x] Validate every entry name; reject absolute/parent traversal, symlinks,
  hardlinks, special files, duplicate normalized entry paths and excessive
  depth, entries and inflated size. Extract into a private stage and publish
  through Linux atomic no-overwrite rename. Cancellation/failure discards
  the unpublished stage. Adversarial regression fixtures include `../` ZIP
  entries, existing target collision and symlink sources.
- [x] Add targeted hostile TAR symlink/device entries, malicious ZIP paths and
  Windows drive prefixes, duplicate members, forged 4 GiB inflated size,
  truncated ZIP, wrong passwords and mid-extraction cancellation. libarchive
  normalizes some Windows ZIP separators to safe relative paths before the
  final checks; drive prefixes and escapes remain rejected.
- [ ] Expand hostile archive/format fuzzing and password workflows; perform
  complete native Mint acceptance before labelling the release stable. Keyring
  and real-device/network-disconnect acceptance were explicitly deferred by
  the owner, not silently considered passed.

**Gate:** hostile archives cannot write outside the selected destination.
The core extraction invariants and ZIP password round-trips are unit-tested;
wider hostile corpora and real Mint device acceptance are open for stable 0.5.

### 0.6 — Everyday navigation and quick actions

- [x] Extend declarative shell composition to top/bottom chrome, XP-style
  task modules in the sidebar and left/right preview placement; keep old
  TOML theme files compatible and enforce safe structural validation.
- [x] Support skin-selectable collapsible versus flat sidebar modules; XP
  task groups can fold without changing file operations or legacy skins.
- [x] Remember up to 32 absolute UTF-8 tab locations and the active tab across
  app restarts, with CLI path override and an opt-out that clears stored paths.
  Restore without statting possibly offline mounts on the render thread.
- [x] Duplicate, move and reopen closed tabs via one skin-independent tab
  command dispatcher, including Ctrl+PageUp/PageDown switching,
  Ctrl+Shift+PageUp/PageDown reordering, Ctrl+Shift+T reopening and right-click
  tab actions. Copy local/remote history and search into new tabs without
  reusing in-flight workers; bound closed-tab history to 16 session entries.
- [x] Alpha.2 implements two local folder panes (F3), bounded persisted
  Back/Forward navigation and closed-tab recovery, and separate windows
  (Ctrl+Shift+N) that cannot overwrite the primary session. Desktop acceptance
  remains open.
- [ ] Extract remaining file-list, toolbar, sidebar and dialog primitives into
  replaceable shell modules. Alpha.2 adds a common navigation/file quick-action
  dispatcher alongside the existing shared tab dispatcher.
- [ ] Native external drag-out for X11/Wayland and a complete keyboard/mouse
  acceptance matrix. Existing file context menus, toolbar actions, external
  drag-in and alpha.2 internal copy-by-drag share no-overwrite operations.
- [x] Alpha.2 adds a lazy symlink-safe folder tree, clickable details headers,
  existing list/grid layouts, natural/lexical sorting and type filters.
- [x] Alpha.3 handles per-entry ENODEV/ENOTCONN/ESTALE-style metadata failures
  without discarding readable siblings, identifies unavailable mounts, keeps
  surviving folder-tree branches and discovers a direct `/mnt` mount.
  Real Linux Mint/FUSE/device recovery acceptance remains unverified.
- [ ] Complete responsive/accessibility checks for every dialog and real
  error/empty/loading workflows on all seven profiles (core states exist).

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
