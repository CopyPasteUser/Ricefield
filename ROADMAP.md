# RiceField roadmap to 1.0

**Current candidate: `1.0.0-rc.19`.** See [the scope and evidence ledger](docs/1.0-STATUS.md) and [owner acceptance matrix](docs/OWNER-ACCEPTANCE-1.0.md).
This roadmap retains incomplete capabilities explicitly: a candidate version
does not turn unchecked safety, integration or physical-device work into a pass.

**Goal:** a reliable, customizable Linux file manager that can replace
Thunar for supported workflows. Keep Thunar installed until real Linux Mint
acceptance passes. Prioritize data integrity, functionality, stability,
performance, Linux integration, then visual polish.

### ChromeOS · Files identity and differentiation

- [x] Replace the generic Modern control stack with an original ChromeOS
  Files-like toolbar, Material-style view toggles, folder title and location
  search. Keep advanced utilities available only on demand.
- [x] Implement My files, common user folders, shortcuts and mounted/network
  storage as real navigation links. Do not simulate unsupported cloud accounts.
- [x] Draw distinct light-blue solid folder/document vectors, compact modern
  detail headers and narrow-window command layout without asset copying.
- [x] Differentiate the content surface with roomy rounded two-line file rows
  instead of reusing Explorer-era dense details tables.
- [x] Route every toolbar/menu command through shared file actions and local
  write guards; keep settings, previews and transfer history accessible.
- [x] Capture and inspect actual CI GUI screenshots for default, grid, More,
  split, compact and narrow views; publish selected images to the gallery
  (native smoke run `42339732861`).
- [ ] Validate on the owner's Linux Mint desktop (scale, keyboard, network,
  mounted USB, theme transitions and pointer interactions).

### Windows 98 · Classic Explorer identity

- [x] Replace the unrelated navy toolbar with a gray, beveled File/Edit/View/
  Go/Favorites/Tools/Help menu and labeled classic navigation/actions toolbar.
- [x] Keep real address entry with a Go button, history navigation, search,
  original yellow folder/white document vectors and dense sortable details.
- [x] Replace generic stacked buttons with a tree-style Folders rail and real
  local, mounted and network storage. Keep file operations and icon/list views
  under shared safety controls, with advanced tools on demand.
- [x] Preserve narrow-window accessibility, custom TOML composition and
  RiceBowl/window-manager ownership of window decorations. Use no MS assets.
- [x] Inspect and publish native CI screenshots for the full shell, functional
  Tools menu, split, compact and narrow layouts. All 21 pairs of seven rice
  default views exceed the visual-difference threshold (CI `42340656430`).
- [ ] Obtain owner visual-fidelity sign-off against era-appropriate references
  on a physical Linux desktop; screenshots alone do not prove exact fidelity.
- [ ] Check physical Linux Mint scaling, mouse/keyboard focus and NAS/USB
  interactions before considering visual acceptance complete.

### macOS · Finder-inspired shell

- [x] Replace the generic theme with a Finder-like toolbar, address/search
  row, Favorites/Locations sidebar, original vector folder/file icons and
  sortable Name/Date modified/Kind/Size detail columns.
- [x] Wire icon/list toggling, Quick Look, New Folder, More menu, favorites,
  tabs, mounted volumes and network discovery into shared RiceField actions.
- [x] Give the Finder details area an alternating-row presentation, distinct
  from ChromeOS cards and Classic Explorer lines.
- [x] Use narrow two-row toolbar and dedicated search row; retain the safe
  command strip for custom TOML shell configurations. Keep advanced tools
  available on demand without crowding default Finder.
- [x] Preserve Linux remote write guards and desktop/RiceBowl window
  decoration ownership. Avoid proprietary Apple artwork and fake iCloud/AirDrop.
- [x] Inspect and publish six unedited native GUI captures for full, icon view,
  More menu, split, compact and narrow layouts. The GUI smoke explicitly clicks
  the Finder icon-view and More controls; Rust checks, release build and all
  seven-theme GUI smoke tests passed on CI run `42295370012` (`46530bb`).
- [ ] Verify keyboard, interaction, network mounts, scaling, accessibility
  and visual fidelity on the owner's Linux Mint hardware.

### Frutiger Aero · Blue Skies shell

- [x] Build an original sky-and-water masthead, low-overhead glass-shelf dock,
  pearl/mint sidebar, and glossy folder/document vectors without OS assets.
- [x] Route all 14 dock commands through shared safety-aware file/navigation
  controllers; preserve local-only action restrictions on network tabs.
- [x] Add responsive compact/desktop dock variants, readable breadcrumb
  navigation, searchable folder view, favorites and NAS discovery.
- [x] Render wider, two-line aqua-tinted file rows and a calmer large-row
  density instead of the ordinary Explorer details renderer.
- [x] Keep advanced tools accessible on demand while maintaining a clean file
  view and no idle transfer strip. Keep window decoration in RiceBowl/WM;
  exercise the Tools dock button in native GUI smoke.
- [ ] Verify hover, keyboard, icon readability, network mounting and real
  performance at 100%/125% scaling on the owner's Linux Mint machine.
- [ ] Compare original artwork against licensed Frutiger Aero-era visual
  references in person; no claim of pixel-identical reproduction.

### Windows 10 · Explorer ribbon shell

- [x] Build separate File/Home/Share/View ribbon pages with stateful commands,
  original vector glyphs, keyboard-accessible buttons and collapsed/compact
  layouts. Dispatch file operations through the shared safety layer.
- [x] Add Quick access / This PC / Network navigation, breadcrumb/search row,
  sorter-aligned details view, flatter folder/document icons and view switches.
- [x] Preserve custom TOML shell composition, network read-only rules and
  RiceBowl-owned window decorations; avoid proprietary Windows artwork.
- [x] Inspect and publish unedited GUI smoke captures for full, split, compact,
  narrow, View and Share layouts. The CI run for commit `5ad5962` passed,
  including scripted View/Share switching, Rust checks and native GUI smoke.
- [ ] Check ribbon focus/selection/menu behavior, keyboard paths, real NAS,
  decorations and display scaling on Linux Mint hardware.
- [ ] Compare visual proportions against licensed Windows 10 references:
  historical pixel-matching is not yet claimed.

### Windows 7 · Aero shell fidelity

- [x] Dedicated breadcrumb address/search row, history, Ctrl+L path editing,
  responsive compact variant and no RiceField-owned window decoration.
- [x] Explorer-like Organize/Share/New folder/View command bar, keyboard-safe
  actions, Favorites, Libraries, Computer/network sidebar and bottom details.
- [x] Native sortable Name/Date modified/Type/Size columns and original
  layered vector icons; preserve all core file operations and custom TOML.
- [x] Inspect actual Xvfb screenshots for full/compact/narrow/split layouts,
  then update `docs/screenshots/win7*.png` from a successful GUI smoke run.
- [ ] Check icon proportions, typography, focus states, scrolling, search,
  selection and native window-decoration integration on Linux Mint in person.
- [ ] Historical pixel-matching against a licensed visual reference: the
  current original-assets UI is inspired by Windows 7, not a certified 1:1 copy.

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
| P0 | Network writes before 1.0 | Server-enforced atomic no-overwrite publication for each promised backend, private staging and byte verification, adversarial two-client races, abort/offline/crash recovery; fail closed when unsupported |

| P1 | Archives | ZIP, TAR variants, 7z and optional RAR with traversal, symlink and bomb protection |
| P1 | Explorer functions | Split view, session restore, drag-out, folder tree, context actions and robust keyboard interaction |
| P1 | Preview and search | Bounded thumbnails, PDF/media/code/archive preview, metadata, filters and cancellable search |
| P1 | Linux integration | MIME/Open With, real URI opening, FileManager1, portals, X11/Wayland and i18n |
| P1 | Composable rice architecture | Versioned TOML shell composition, shared actions, independently replaceable UI panels; no core rewrite |
| P2 | RiceBowl polish | Seven distinctive shells, live theme IPC, editor, custom Rust component registration and accessibility refinement |
| P2 | Organization extras | Saved searches, duplicate detection, optional index/tags, ACLs and extended attributes |

WebDAV has a conditional `PUT`/`MOVE Overwrite: F` implementation with a
disposable live server race probe, staged byte verification and a private,
synced stage/destination journal reserved before its first upload PUT. Uncertain
results remain available after restart for explicit *local-only* record
acknowledgement, not automatic server cleanup. This is **partial P0 work**:
SMB/SFTP/MTP backend guarantees and actual NAS/offline/crash acceptance are
outstanding. Never substitute generic GIO move for any of them.
The [backend-specific capability matrix](docs/REMOTE-BACKEND-MATRIX.md)
records each supported/read-only protocol and the actual-process stage-PUT
crash regression separately from unverified NAS/hardware semantics.

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
- [x] Harden the Properties POSIX permission editor against symlink-swap races
  with descriptor-bound chmod. For owners lacking read permission, use
  O_PATH and Linux fchmodat2 with AT_EMPTY_PATH where supported (rc.12).
  RC.13 additionally records the selected inode's metadata in the Properties
  snapshot, and refuses chmod if that exact file changed or was replaced
  before Apply. Full ACL and privileged ownership editing still require separate work.
- [x] RC.14 adds basic UID/GID ownership changes through descriptor-bound
  `fchownat(AT_EMPTY_PATH)` on Linux. The Properties selection is checked
  again before mutation; the kernel handles normal permission and group
  membership checks. There is no sudo/privileged helper. GUI support for
  complex POSIX ACL rules and privileged ownership changes remains open.
- [x] RC.15 offers bounded creation of `user.*` text extended attributes on
  ordinary local files/directories. An O_NOFOLLOW file descriptor pins the
  selected inode and `XATTR_CREATE` rejects every existing name atomically,
  including races. There is no ACL editor, attribute replacement/deletion,
  privileged namespace writer or guarantee that every filesystem supports
  xattrs. Ordinary kernel permissions and disposable Mint testing apply.
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
  preflight and safe confirmation are implemented; complete failure recovery.
  RC.7 adds bounded, private, durable **read-only result summaries** across
  restarts via a single background writer. A later increment retains up to 24
  bounded per-item source/target/error summaries per completed operation,
  prioritizing failed, cancelled and unstarted files; older v1 ledgers remain
  readable. The worker drops oldest records when the 256 KiB file limit is
  reached. Waiting jobs, full per-item records and Undo are never replayed or
  restored. Non-regular, corrupt or unsupported history is preserved unchanged.
  Full crash recovery and untruncated durable per-item histories remain open.
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
- [x] Give desktop clipboard subprocesses a three-second process deadline on
  both read and write. Anonymous bounded Linux memory files replace pipes,
  avoiding a blocked writer/reader or an unbounded output allocation. A hung
  helper is killed and the in-app clipboard remains available; X11/Wayland
  helper compatibility still requires real-session acceptance.
- [x] Read standard `text/uri-list` desktop clipboard targets after trying
  the GNOME file-copy format on both X11 and Wayland. URI-list imports are
  always Copy, never an inferred Cut; remote or malformed URIs remain rejected.
  Other desktop-manager interoperability still requires real-session tests.
- [x] Remove per-frame filesystem probes from ChromeOS and macOS sidebars.
  Their standard folders now use the same asynchronously refreshed available-
  places snapshot as the other five rices; a stalled filesystem must not block
  the renderer simply because its Places section is visible.
- [x] Move periodic external TOML theme fingerprinting and reload/parsing off
  the egui render thread. A single pending worker at a time avoids repeated
  scans on stalled mounts; generation and path checks discard late results
  after changing themes, and changed-during-read sources preserve the active
  theme. Startup now paints a bundled fallback immediately and sends initial
  external TOML parsing/fingerprinting through the same guarded worker. A
  suspended FUSE/NAS reader can keep that optional load pending indefinitely,
  but cannot block the first GUI frame. Real slow-mount acceptance remains open.
- [x] Persist preference changes on one background writer instead of running
  filesystem writes in the render loop. A bounded single-slot snapshot holds
  the newest settings while the writer is busy, avoiding stale writer races or
  an unbounded queue. Failures are shown in the GUI and normal shutdown waits
  up to two seconds for pending writes; sudden process death or indefinitely
  stalled state mounts still cannot guarantee persistence.
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
  bounded reads. The verified cross-device copy path now also creates durable
  journal records in production: Prepared → StagedVerified → Published, then
  retires Completed records. A crash or failed publication leaves inspectable
  evidence, while a successful copy still retains the original source. Full
  resume/rollback and power-loss validation remain open.
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
- [x] Allow an explicit, cancellable, two-step confirmation to retire the journal for a
  **previously published independent copy** after a second bounded comparison
  and refreshed source/destination inode evidence. Linux directory copies
  additionally require strict nested metadata and hardlink-topology checks. Serialize
  journal writes across workers and reject stale writers so a retired record
  cannot be resurrected. Keep both source and destination untouched; refuse
  stale metadata, altered bytes, collisions and unverified staging. A directory
  record remains available when nested metadata cannot be verified. This is
  not automatic replay, cleanup of staging debris, or full crash recovery.
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
- [x] Exercise the journal with actual abrupt child-process exits at Prepared,
  StagedVerified, atomic publication-before-Published and Published checkpoints.
  A new process independently inspects the durable state, staged-to-published
  inode evidence and intact source bytes. This is crash-process fault injection,
  **not** simulated sudden power loss, automatic recovery or safe source cleanup.
- [x] Reclaim structurally valid `Completed` journal entries only when the
  bounded journal capacity is exhausted, validating with the same no-follow
  reader used for recovery. Keep incomplete, malformed or changing records,
  and sync the journal directory after cleanup. No incomplete record expiry,
  resume, rollback or comprehensive long-term retention policy is implied.
- [x] Retain a fully verified staging payload on later journaled publication
  failure, preserving original and competing destinations. Provide independent
  read-only source/staging comparison in recovery review; incomplete stages
  remain disposable. Publication of eligible staged files/links/trees now has an
  explicit guarded workflow; all other cases still require manual resolution.
- [x] Permit explicit, two-step Linux restart publication of a fully verified
  staged regular file/symlink/bounded directory only after fresh source/stage
  comparison (including nested metadata and hardlinks for directories),
  private-stage inode checks and atomic no-overwrite rename; sync and verify
  the published result, retain the source and fail closed on any uncertainty.
  Oversized/unverifiable trees, rollback, power-loss and larger copy verification
  remain outstanding.
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
  Individual completed Rename actions and fully completed Batch Rename actions
  now offer session-only Undo in Transfers, guarded by post-rename snapshots,
  complete preflight and no-overwrite semantics. Broader Undo (especially
  partial batches, copies, cross-device moves and permanent delete) remains
  open and cannot rely on stale history.
- [x] Implement bounded 2–256 item batch rename with a live prefix/suffix
  preview, before-start collision preflight and no-overwrite renames. Preserve
  non-UTF-8 filename bytes; test collisions and mid-batch partial failures.
- [x] Add zero-padded numbering and literal search/replace to the complete
  preview without new dependencies. Refuse search/replace of non-UTF-8 names.
- [x] Implement bounded safe batch rename cycles and best-effort guarded
  rollback for ordinary errors. RC.8 uses Linux atomic name exchange for
  selected-name cycles, no-overwrite moves for chains, and refuses changes to
  unrelated targets. Test cancellation, external modifications and selected
  hardlink aliases. RC.9 adds explicit session-only batch Undo with a complete
  post-operation identity check, including swaps/cycles. A crash may leave
  partially permuted names (without dropping entries); durable batch Undo and
  power-loss recovery remain open.

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
  **P0 write safety:** CLI `gio move` can replace existing targets, and native
  `g_file_move` without overwrite flags exhibited a concurrent publish race.
  See [the remote write safety decision](docs/REMOTE-WRITE-SAFETY.md) and
  closed, unmerged [PR #71](https://github.com/CopyPasteUser/Ricefield/pull/71).
  Never enable uploads based on GIO's flags alone.
- [ ] Discover drives/volumes/mounts and their capabilities dynamically.
  Existing background mountinfo enumeration now labels detected network mounts
  separately in the Volumes sidebar. RC.10 adds throttled UDisks2 ObjectManager
  discovery of unmounted external/removable filesystems, with strict filtering
  and fallback when the system bus is unavailable. Rich device capabilities,
  LUKS and internal fixed drives still require further integration.
- [ ] Mount/unmount/eject/unlock through OS APIs and Polkit.
  The Volumes sidebar can now invoke bounded background GIO unmount/eject
  commands after confirmation. RC.10 adds explicit native UDisks2 mounting of
  discovered removable filesystems through system Polkit authorization. RC.11
  binds mounted USB paths to discovered device objects, adds native UDisks2
  unmount/eject with fresh-state checks, and retains GIO for other mounts.
  Encrypted drive unlocking, portal integration and real-hardware acceptance
  remain open. No forced unmount is offered.
- [x] RC.16 adds opt-in UDisks2 USB PowerOff when explicitly advertised. The
  backend re-queries all sibling filesystems, including ignored/hidden block
  objects, and refuses if any are mounted or metadata is incomplete. UDisks2
  handles Polkit, final busy checks and flushing. LUKS and physical USB/power
  safety acceptance remain open; other buses cannot be powered off here.
- [ ] Connect SMB/SFTP/WebDAV, browse system NFS and supported MTP devices.
  Initial non-blocking `gio mount` connection attempts are supported without
  URI credentials. Read-only remote listing works in normal tabs; native remote
  file operations are not yet available. Alpha.6 adds a limited interactive
  username/domain/password flow through the GIO console mount operation;
  real-device verification and native mount APIs remain outstanding. Alpha.8
  adds passive SMB network discovery through existing GVfs/Avahi services;
  missing advertisements still require typing an address once. A
  bounded metadata listing distinguishes traversable directories from files
  that cannot yet be opened or transferred. Small UTF-8 remote files can now
  be inspected through a bounded, cancellable, read-only text preview. A later
  preview increment also permits a first-32-KiB prefix for larger remote files
  and renders binary data as a bounded 256-byte hex/ASCII view. BOM-marked
  UTF-16LE/BE files now use the same safe text decoder as local previews,
  trimming an incomplete code unit at the remote prefix boundary. This is not
  lossless network content validation or a full file-open operation. RC.2
  added staged local single-file downloads; rc.4 adds bounded whole-folder
  content downloads without exposing incomplete local trees. Native high-volume
  network transfers and remote writes remain open. RC.5
  rejects symlink/special-file downloads from GIO listings, including inside
  recursively downloaded folders, instead of following untrusted links. RC.6
  introduces up to 16 explicit waiting network downloads, snapshotted target
  folders, FIFO dispatch, remove/cancel/pause/resume and pause-on-error. Waiting
  jobs are session-only and never automatically replayed after a restart.
  A subsequent read-only, explicitly requested staging inspection can list
  private abandoned download directories without deleting or resuming them;
  it is not a verified restore protocol or replacement for crash testing.
- [ ] Connect dialog, secure credential handling, favorites and reconnect.
  A basic URI dialog, 32 validated persistent URI favorites and manual
  reconnect are implemented. Alpha.6 supplies a masked one-attempt credential
  prompt; alpha.8 adds an explicitly selected Secret Service keyring save,
  lookup and forget flow and automatically favorites successfully connected
  URIs. Actual Mint keyring/NAS tests, unattended reconnect and recovery from
  mid-operation disconnects remain open.

**Gate:** offline shares, disconnections, bad credentials and removed USB
devices produce clear errors without frozen UI or silent data loss.

**Owner decision (2026-11-04, updated 2026-11-05):** The owner now explicitly
requests optional saved NAS logins and automatic *discovery* of advertised
servers. Alpha.8 stores selected credentials only in the user's Linux Secret
Service after successful connection; settings never store the password. No
password persistence occurs by default, and no automatic login is attempted.
Real-device, unplug, keyring, and disconnection acceptance are still open.
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
- [x] Extend those fixtures with 40 deterministic generated ZIP/tar.gz archives
  containing randomized absolute, parent-traversal and drive-prefixed paths.
  Verify inspection and staged extraction cannot publish their contents.
  This is a reproducible hostile corpus, not continuous mutation fuzzing or a
  substitute for the full format/device acceptance gate.
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
  dispatcher alongside the existing shared tab dispatcher. Alpha.4 moves
  file-cell painting into a presentation-only renderer. Alpha.5 also extracts
  file-cell responses/sort headers, semantic toolbar actions, sidebar section
  frames and shared dialog framing. Full skin-specific override contracts and
  remaining complex UI bodies are still open.
- [ ] Native external drag-out for X11/Wayland and a complete keyboard/mouse
  acceptance matrix. Existing file context menus, toolbar actions, external
  drag-in and alpha.2 internal copy-by-drag share no-overwrite operations.
  Alpha.6 adds a context-menu bridge to optional installed `ripdrag` or
  `dragon` native drag-source helpers. Direct tile-to-app dragging still
  needs framework support; eframe 0.37 / winit 0.30.13 cannot initiate it.
  X11/Wayland compositor acceptance is open.
- [x] Alpha.2 adds a lazy symlink-safe folder tree, clickable details headers,
  existing list/grid layouts, natural/lexical sorting and type filters.
- [x] Alpha.3 handles per-entry ENODEV/ENOTCONN/ESTALE-style metadata failures
  without discarding readable siblings, identifies unavailable mounts, keeps
  surviving folder-tree branches and discovers a direct `/mnt` mount.
  Real Linux Mint/FUSE/device recovery acceptance remains unverified.
- [ ] Complete responsive/accessibility checks for every dialog and real
  error/empty/loading workflows on all seven profiles (core states exist).
  Alpha.4 adds compact-window, dialog Escape and keyboard tab-action Xvfb
  smokes across eight profiles, plus separate-window startup in Windows 7;
  alpha.5 adds stacked navigation, constrained common dialogs, visible
  split/dialog assertions and 380px Xvfb captures. The full real-device
  pointer, focus and screen-reader matrix remains open; see
  [Mint acceptance matrix](docs/0.6-MINT-ACCEPTANCE.md).

**Gate:** important local workflows work by mouse and keyboard in all themes.

**Alpha.7 (2026-11-05):** Source and bundled installs now register a
desktop-menu launcher with an absolute binary path, XDG data-directory support
and isolated installation checks. Real Mint app-menu launch awaits owner
verification. `/mnt/medien` still fails with Linux ENODEV in the owner's
screenshot; we need `findmnt`/`ls` evidence before diagnosing or changing the
live mount. The owner explicitly **did not approve stable 0.6 acceptance**.

**Alpha.8 (2026-11-05):** Passive SMB/GVfs and Avahi NAS discovery, browsable
server share entry points, explicit keyring save/load/forget and automatic
URI-only favorites are implemented for preview. The broken `/mnt/medien`
system mount is not silently remounted; use `smb://server/share` through GIO
while diagnosing the mount separately. Discovery and keyring acceptance still
need the real Mint session. The stable 0.6 gate remains **not approved**.

### 0.7 — Previews and metadata

- [x] Bound image preview input to 32 MiB on the opened file descriptor,
  check the target of symlinks, reject special files and retain the 16 MP
  pixel limit; add preview safety regressions.
- [x] Bounded 96-entry thumbnail cache with a single background decoder,
  48-slot newest-visible priority queue and 12 MiB / 16 MP input caps.
- [x] Space-to-toggle Quick Look and bounded read-only image/PDF/text/code/
  audio/video/archive preview; PDF and media metadata use optional system
  tools with timeouts. Native video/audio playback is delegated to Open.
- [x] Offer a bounded local binary hex/ASCII preview (256-byte maximum)
  for control-bearing or invalid UTF-8 data, escaped before display and
  read through the existing background preview worker.
- [x] Identify supported raster images from a short signature even if their
  filename has no image extension, and decode BOM-marked UTF-16LE/BE text
  without rendering unsafe control characters. Unsupported formats still
  receive a bounded informational or hex preview, never arbitrary decoding.
- [x] Read-only metadata, on-demand bounded folder size and SHA-256, and
  Linux extended-attribute names / POSIX ACL presence (not ACL editing).

**Gate — open:** malformed/huge-file fuzzing, performance on slow drives,
optional PDF/media backends and visual Mint acceptance must pass before
marking 0.7 stable. The 0.6 NAS/mount acceptance is still blocked separately.

### 0.8 — Search and organization

- [x] Cancel obsolete local folder scans and recursive filename searches on
  new requests, tab closure and query changes; reject queued stale results.
- [x] Add lightweight type/size/date presets to the same local filter policy;
  remove hidden selections before any further operation. No indexing daemon.
- [x] Bounded, cancellable, opt-in local UTF-8 content search; no index,
  symlink traversal, special-file reads or unbounded scans (0.8 alpha.2).
- [x] Custom filename glob plus size and modified-age ranges (0.8 alpha.3),
  combined with type/size/date presets and safe selection pruning.
- [x] Bounded Unicode filename regex alongside glob, with inline syntax errors,
  persisted settings and adversarial-pattern regressions (1.0 rc.1).
- [x] Bounded cancellable network filename search (0.9 alpha.4): explicitly
  opt-in, read-only recursive GIO listing, no index or remote file-content
  reads; optional indexing remains a post-1.0 enhancement.
- [x] Opt-in, capped saved local searches with private atomic preferences
  (0.8 alpha.4); no query persisted or automatically rerun without consent.
- [x] Bounded read-only duplicate candidate finder for visible local files,
  with same-size SHA-256 grouping and manual-review-only UI (0.8 alpha.5).
- [ ] Additional batch action previews (batch rename and paste already have
  planning/preview steps); performance acceptance for huge folders.

**Gate:** 100k-entry folders and recursive searches remain responsive.
Automated synthetic sorting/glob stress covers 100,000 entries without
touching disk; real HDD and UI responsiveness measurements remain outstanding.

### 0.9 — RiceBowl and Linux desktop integration

- [x] Parse validated remote CLI arguments and desktop `%U` file/URI launch
  requests through the existing typed Location/GIO backend; reject URI
  credentials while keeping native non-UTF-8 local path handling. FileManager1,
  portals and full clipboard interoperability remain open.
- [x] Local MIME Open With through GIO's desktop association registry with
  bounded background discovery and argument-safe launching (0.9 alpha.2).
- [x] Opt-in native FileManager1: folder requests, local item selection and
  sequential properties, private-bus protocol/ownership and GUI checks (1.0 rc.1).
- [ ] Portals and complete URI/clipboard interoperability.
- [x] Live reload for an explicitly supplied `--theme-file` TOML with
  256 KiB validation, previous-theme fallback and manual retry (0.9 alpha.1).
- [x] Visual TOML theme editor, import, private no-overwrite export and
  persisted activation (0.9 alpha.3).
- [x] Visual shell composition controls: place, hide and reorder the shared
  toolbars; select shell kind, sidebar groups and preview position without
  altering filesystem operations (0.9 alpha.5).
- [ ] Direct RiceBowl event/theme sync.
- [ ] Seven distinct shells and matching icons, controls and layouts.
- [x] Automated structural distinctness for Windows 98, Windows 7 and macOS,
  plus Xvfb screenshot-difference gates and a custom top/bottom composition.
- [ ] Confirm per-shell visual fidelity on real Mint displays and support
  custom Rust components via a compile-time registry, without an unsafe
  runtime plugin loader.
- [ ] German/English localization, HiDPI, multi-monitor, accessibility review.

**Code gate:** all supported shell behavior and automated visual regressions
pass. Physical Mint X11/Wayland review is deliberately scheduled *after* the
1.0 code-complete candidate, before stable release sign-off.

### 1.0 — Code-complete candidate, then owner acceptance

- [x] Prepare an explicit owner test matrix with reproducible disposable
  fixtures, per-case pass/fail IDs, Linux Mint/NAS/USB/RiceBowl/low-memory
  coverage and a results template in
  [OWNER-ACCEPTANCE-1.0.md](docs/OWNER-ACCEPTANCE-1.0.md). The tests have
  **not** been performed on the owner's physical devices.

- [x] Serialize concurrent user-local installer, rollback and uninstall
  operations with a shared advisory lock and automated overlapping-process
  tests. Power-loss and physical Mint validation still required.
- [x] Add automated isolated clean-install, upgrade, injected registration
  failure, atomic binary replacement and installer rollback tests; enable
  `v1.0.*` artifact workflows without publishing a tag. Real-device update,
  uninstall and power-loss verification remain open.
- [x] RC.17 stages desktop icon replacements on the same filesystem and
  atomically renames only a complete image into view; a controlled slow-copy
  regression checks that the previous icon remains readable during the copy.
  Multi-file power-loss recovery is still outstanding.
- [x] Format, Clippy, unit/integration/UI/security tests pass in CI (rc.3:
  [run #481](https://github.com/CopyPasteUser/Ricefield/actions/runs/42107821509),
  258 unit/regression tests, separate D-Bus test, eight GUI profiles).
- [ ] Measure startup, idle RAM/CPU, 100k entries, HDD and large transfers.
- [ ] Prepare a code-complete `1.0.0-rc` candidate with current README,
  changelog, representative real screenshots and verifiable release artifacts.
- [ ] Resolve known critical code defects and provide an explicit test matrix
  **only once the candidate is code-complete**, not during alpha development.
- [ ] **After the 1.0 code freeze**, have the owner run the provided tests on
  4 GB hardware, network disconnect, NAS, Wayland/X11, USB removal, install,
  update, uninstall, rollback and recovery in a safe lab.
- [ ] Publish stable `v1.0.0` only after those owner results are reviewed and
  critical failures fixed. Keep Thunar available until then.
- [ ] Verify that a new rice can be assembled from configured components
  without changing the file-management core or interrupting transfers.

**Definition of code-complete:** dependable daily file management for
explicitly supported protocols, reviewed recovery and failure paths, automated
tests, accurate docs and measured CI/lab performance. This is **not** the same
as stable release: owner/manual acceptance starts afterward, as requested.
