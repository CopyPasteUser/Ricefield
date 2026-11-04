# Changelog

## Unreleased

- Restore validated, read-only SMB/SFTP/WebDAV/MTP tabs alongside local tabs
  in the existing 32-entry session snapshot, preserving selection and active
  tab. Legacy local-only settings remain readable. Reject credentials and
  malformed or oversized remote URIs; network scans stay asynchronous and
  no remote file mutation is enabled.
- Read GIO entry types in a single bounded background listing, so directory
  entries are navigable while remote files remain visibly non-openable until
  the safe file backend exists. Avoid turning file clicks into failed scans.
- Allow read-only remote URIs in regular tabs with per-tab navigation/history
  and asynchronous GIO scans. Keep remote entries separate from local POSIX
  entries and block mutating local actions on remote tabs.
- Add a bounded, read-only GIO network browser accessible from remote address
  entry or a completed connection attempt. Decode no untrusted URI into a
  local path; cap listing output and item count, reject unexpected entries,
  cancel superseded scans and show timeout/offline errors without blocking
  the GUI. Add safe persisted
  URI favorites and an explicit reconnect control. This does not
  enable remote file mutations.
- Add a GIO-backed, non-blocking network connection attempt and confirmed
  unmount/eject actions to the Volumes sidebar. Commands are argument-based,
  have a bounded runtime, never call sudo, and do not provide interactive
  authentication.
- Label mounted network shares separately from local volumes in the sidebar.
  Volume detection alone does not confer mount or eject permission.
- Start 0.4 location handling with a shared typed local/remote address parser
  for the CLI and GUI path field. Recognize SMB, SFTP, WebDAV and MTP URIs,
  reject embedded credentials/invalid encodings without scanning a fake local
  folder. Existing `file://` and raw non-UTF-8 local pathname behavior remains
  unchanged.

## 0.3.0-alpha.4 — 2026-11-04

- Make cross-filesystem Move **source-retaining by default**: verified staged
  Copy with atomic no-overwrite publication, followed by **no automatic
  deletion**. Show "Copied — source kept" per item, tell users to inspect the
  destination and manually Trash the original, and consume the completed Cut
  intent. Same-filesystem atomic renames are unchanged. No new destructive
  Move journal is created; legacy v1–v3 unfinished records remain inspectable.
  Regression-test source retention, concurrent source additions and clipboard
  reconciliation. This removes the dangerous final unlink race from the new
  cross-volume path, not all transfer/recovery risks.
- Upgrade the durable cross-device Move journal to v3 while keeping v1/v2
  records readable. Save the verified staging payload's inode identity before
  publication and refuse the Published transition if the destination is not
  that same inode. Crash-recovery review now reports the staging-to-destination
  identity relationship, including publication that happened before the
  Published record could be written. This is provenance evidence, **not**
  proof of unchanged bytes, safe automatic resume or source deletion.
- Reject cross-device Move before staging when a regular-file inode also has
  hardlink names outside the selected tree, or when a hardlinked symlink cannot
  be reproduced. Internal regular-file hardlinks remain preserved. This avoids
  silently severing external hardlink relationships; ordinary Copy remains
  available for intentionally creating independent copies.
- Offer session-only Undo for successfully renamed individual files/folders
  through the shared Transfers history. Capture the original raw filename and
  post-rename inode/metadata snapshot; restore with Linux's atomic
  no-overwrite rename only when the moved entry still matches that snapshot.
  Refuse modified/replaced entries and occupied original names without
  touching them. Undo is never replayed after restart and does not yet cover
  partial batch renames, Copy/Move, Trash or Delete.
- Offer an opt-in fresh Copy preview from the unfinished-move review when the
  original source's recorded identity still matches. It reuses the normal
  background preflight, defaults to Keep both and requires a second explicit
  Start transfer action. It never retries a destructive Move or clears the
  original recovery record; replaced, changed, missing and legacy source
  identity states cannot use this shortcut.
- Recovery review now displays recorded source and destination inode/metadata
  observations separately, including missing evidence and replacement warnings.
  Browse the containing source, destination or staging folders directly from
  the review window without following the recorded entry as a symlink. These
  are navigation and read-only diagnosis aids, **not** restart resume,
  rollback, source deletion or clearance of journal evidence.
- Add a fourth non-destructive Paste conflict policy, **Skip only matching
  contents**. Compare colliding files, directories and symlink targets using
  bounded, cancellable, no-follow reads (512 MiB / 10,000 entries); accept a
  skip only after a complete match. Different, unavailable or too-large
  comparisons fail the whole preflight before any transfer. Preserve cut
  sources when a collision is skipped, reject two incoming items targeting
  the same new name, and repeat verification on the worker after preview.
  This is a read-only observation, not atomic protection against concurrent
  writers, verified replacement, or permission to delete the original.

## 0.3.0-alpha.3 — 2026-11-04

- Add a manual Pause / Resume toggle for pending Copy jobs in the shared
  Transfers view. Running work continues; only not-yet-started queue entries
  wait. Label the paused state consistently across rice shells.
- Use a process-local atomic sequence and collision retry for file-operation
  test directories. Parallel fixtures no longer depend on distinct clock
  nanoseconds, eliminating an intermittent `AlreadyExists` test failure.
- Reclaim only validated terminal `Completed` move journal records when the
  4,096-record capacity would otherwise block a new move. Share the bounded,
  no-follow record validator with the recovery reader; keep malformed,
  replaced and unfinished evidence untouched. Sync the journal directory
  after retirement. This is capacity maintenance, not safe restart replay.
- Track every symbolic-Link operation in Transfers, with a read-only batch
  destination preflight, reserved unique names, item-count progress and
  individual success/failure/cancel outcomes. A racing external destination
  cannot be overwritten; earlier completed links remain visible after a
  partial failure. Add tests for duplicate basenames, collisions and cancel.
- Add byte-reproducible Linux x86_64 release bundles with SHA-256 manifests,
  binary-version guards and an unprivileged installer. Validate archive
  equivalence, checksums and install/uninstall behavior in isolated fixtures.
  Provide a manually dispatchable CI packaging job and tag-gated GitHub
  prerelease/publication workflow; do not tag final 0.3 before the outstanding
  recovery and Linux Mint acceptance gates pass.
- Serialize the recovery-journal capacity check and creation across cooperating
  Unix RiceExplorer processes with an advisory directory lock. Open that
  directory without following symlinks and validate its private permissions.
  A concurrent-writer regression verifies that only one process can reserve
  the final journal slot. Add restart fault injection for an already-published
  destination whose journal stopped at StagedVerified; never infer permission
  to delete or resume from the destination's presence.
- Record the original source's Unix inode/metadata identity before staging and
  the published destination's identity after its durable publication barrier.
  Version 2 recovery records remain inspection-only: they can flag a replaced
  inode or changed metadata but never authorize automatic deletion, resume or
  rollback. Continue reading legacy version 1 records without inventing
  missing provenance. Reject malformed v2 evidence and refuse new moves when
  the journal is at its 4,096-record scan limit; preserve every incomplete
  record instead of silently pruning it. Add restart/identity/capacity tests.
- Preserve Linux POSIX access/default ACLs when staging regular files and
  directories. Apply ACLs after chmod to retain their masks; remove ACLs
  inherited from the private staging hierarchy when the source has none.
  Attempt to keep original uid/gid (including symlink ownership) when the
  process has permission, while allowing ordinary Copy to use the current
  owner otherwise. Strict cross-device Move verification still refuses
  ownership mismatches. Do not copy privileged security labels/capabilities.
  Add real ACL fixtures and inherited-ACL regression tests.
- Fail closed on cross-device Move if copied Unix ownership, group, mode or
  modification timestamp differs from the original. On Linux, verify every
  visible extended-attribute name and value, including ACL/security namespaces
  that RiceExplorer cannot yet copy. Refuse publication or source cleanup
  when metadata is lost or unreadable; byte equality alone is insufficient.
  Add permission- and xattr-loss regressions. This does not solve atomic
  source unlink or implement privileged metadata migration.
- Run desktop file clipboard copy/cut and paste helpers off the render thread.
  Coalesce queued copies, limit external payloads to 4 MiB and give desktop
  reads a two-second UI deadline. Ignore late paste responses and cancel a
  pending paste when its tab or folder changes, rather than moving old files
  into an unexpected location. A stuck provider cannot spawn unbounded readers.
- Refuse to reuse a staged hardlink if its source inode changed after the
  first link name was copied; this prevents publishing stale content under
  another name in a mutable source directory.
- Pin private recovery journal records with a no-follow, nonblocking open,
  then recheck the opened inode and its path after reading. Replaced links,
  FIFOs and concurrently changed records fail closed instead of redirecting
  or stalling a startup scan. Add deterministic replacement regressions.
- Final v0.3 safety gate, Mint hardware acceptance and recovery decisions are
  still in progress. Do not treat this alpha as a production release.
- Load the Properties dialog in a single background worker with a latest-only
  pending request; reject stale-path responses and disable permission editing
  until metadata is available. A dead mount no longer blocks drawing every
  frame. Add regressions for correct/stale responses and source metadata.
- Fault-inject a competing destination immediately before atomic publication,
  plus a source change after verified staging. Both fail without losing the
  source, overwriting the competitor or leaving temporary staging content.

## 0.3.0-alpha.2 — 2026-11-04

- Verify regular-file source and published-destination identities through
  `O_NOFOLLOW | O_NONBLOCK` descriptors, checking both the opened inode and
  original path after reading. Reject swapped symlinks, replacement inodes
  and FIFOs without following the substitute or waiting indefinitely.
- Recheck source and destination directory/link identities after recursive
  byte verification instead of assuming the earlier path remains stable.
- Preserve Linux symbolic-link access/modified timestamps with `utimensat`
  and `AT_SYMLINK_NOFOLLOW`, including broken links; reject a staged copy if
  the destination filesystem cannot preserve link timestamps.
- Add regression coverage for broken-link timestamps, symlink/inode swap
  detection and nonblocking FIFO rejection. Cross-device source cleanup is
  still not an atomic compare-and-unlink operation.

## 0.3.0-alpha.1 — 2026-11-04

- Read the actual package version from Cargo in `--help` and `--version`
  instead of showing the old hard-coded 0.2 text; exercise both in CLI smoke.
- Copy accessible `user.*` extended attributes before restoring restrictive
  source permissions, so read-only source files retain their metadata.

- Preserve accessible `user.*` extended attributes on Linux while staging
  local copies and cross-filesystem moves. Fail safely if the destination
  cannot store a source attribute; privileged labels, ACLs and ownership
  remain explicitly unsupported.
- Preserve internal hardlink relationships when copying a directory tree:
  repeated source inodes are hardlinked in private staging instead of creating
  unrelated independent files. The optimization is scoped to one published
  tree; cross-item and external hardlink identities remain unsupported.
- Before cleaning a published cross-device Move, take a bounded source-tree
  identity manifest and recheck each entry. Refuse substituted files even if
  their bytes match the published copy. Account for inode ctime changes
  caused by RiceExplorer's own earlier hardlink removals. This narrows but
  does not eliminate the remaining non-atomic check/unlink race.
- Add an explicit read-only source/destination comparison in the unfinished
  move inspector. Compare regular-file bytes and nested directory names,
  symlink targets and object types on demand; run in a cancellable worker
  with 512 MiB / 10,000-entry / 128-level caps. Mark missing, unreadable,
  changed or oversized trees as inconclusive; never automatically delete,
  resume or dismiss records based on a matching snapshot.
- Extend read-only crash-recovery inspection to observe source, published
  destination and staging path status asynchronously. Distinguish missing,
  present, symlink and inaccessible entries, and present conservative guidance
  without attempting retries or deletion. Coalesce repeated scans while an
  earlier scan is still running, avoiding unbounded worker threads on slow
  or disconnected storage.
- Sync the staging directory before marking its contents verified and sync
  the destination parent after atomic no-overwrite publication. A failed
  post-publication directory sync leaves the original source intact for a
  cross-device Move and reports that the published destination may need
  manual inspection. Inject failures at both sync barriers in regression
  tests; neither is allowed to remove the original source.
- Preflight complete multi-item Trash and permanent Delete requests before
  removing any paths. Reject missing, duplicate and overlapping selections;
  revalidate each selected source at execution time. Surface per-item
  outcomes and count-based progress, including explicit not-started entries.
  List exact selected paths in both confirmation dialogs before proceeding.
  Earlier deletions remain irreversible if a later runtime failure occurs.
- Strengthen source identity checks with Unix inode change time (ctime), so a
  same-sized in-place rewrite with a restored modification time is refused.
- Show transfer elapsed time, average transferred-byte speed and approximate
  ETA in all rice layouts, with per-job durations in session history. Avoid
  misleading estimates on very short or unknown-sized transfers.
- Preflight full Copy/Move batches before the first mutation: validate source
  identities, destination directories, duplicate and nested selected sources,
  and destinations inside a source tree. Reserve names within a batch so two
  same-named inputs cannot unexpectedly collide. Reject sources that change
  after preflight before their turn begins. Keep atomic no-overwrite writes.
- Show a background-computed, row-virtualized Paste conflict preview with
  intended paths and rename/skip totals. Add a *Require no conflicts* mode
  that refuses the entire batch when a destination name is already occupied.
  Revalidate on the worker; preview is advisory, not a filesystem lock.
- After a partially failed cut-and-paste, retain only uncompleted sources for
  retry in the internal clipboard, without replaying already moved paths.
- Track detailed Copy/Move outcomes for each requested source, including
  destination paths, cancelled/failed items and later not-started items.
  Show expandable, row-virtualized per-file results in the shared Transfers
  window. Surface preflight errors without falsely implying earlier files
  were moved; preserve skipped cut sources in the internal clipboard for
  selective retries while ignoring unchanged stale external cut contents.
- Add Paste with options in every rice's shared Actions menu, with a
  no-overwrite Keep both / Skip existing choice for copy and move. Preserve
  skipped source files when moving, report skipped item counts, and account
  for skipped bytes in worker progress. Verify multi-file mixed outcomes and
  collisions with dangling target symlinks. Normal Ctrl+V and Duplicate
  retain Keep both semantics; full replace/compare decisions remain pending.
- Preserve non-UTF-8 filename bytes when choosing a collision-free copy name.
- Capture the original destination when a New Folder/New File dialog opens.
- Correct the desktop launcher to pass local paths, and validate CLI file URIs.
- Extend Xvfb startup/screenshots to all seven bundled themes.
- Add an uninstall script that preserves user configuration.
- Expand the roadmap with P0/P1/P2 priorities and measurable release gates.
- Introduce versioned shell profiles and configurable top-level UI component
  order for all seven built-in rices, reusing the same file operations.
- Let rice profiles compose chrome modules into top and bottom regions, move
  previews to either side and order a real file-tasks module in the sidebar.
  Give XP Luna a task-oriented sidebar and Frutiger Aero a bottom command
  strip; reject duplicated/missing navigation and invalid preview widths.
- Give each skin flat or independently collapsible sidebar modules; enable
  collapsible task/place/favorite/volume groups in XP Luna, while preserving
  legacy flat rendering for other profiles.
- Improve search debounce repaint timing and Linux mountinfo path decoding.
- Cancel obsolete local folder scans and recursive searches as soon as a tab
  navigates, closes or changes its query. Workers check cancellation between
  filesystem entries and do not publish partial/stale results; add regression
  tests for cancellation tokens and interrupted reads.
- Add Copy Path to the actions/context menus and Ctrl+Shift+C.
- Stage local copies and cross-device moves under hidden temporary paths,
  verifying cross-device transfers before atomic no-overwrite publication.
- Allow custom rice profiles to configure sidebar side, width and sections,
  plus visibility of the status bar, without changing the file core.
- Reverify a published cross-device copy before source cleanup, and delete
  only individually checked entries; add concurrent-source-mutation regression.
- Add versioned, private, synchronized operation records for cross-device moves;
  persist the `Published` state before any source removal.
- Inspect unfinished moves asynchronously on launch and after operations. Show
  a visible warning across all themes, with read-only recovery details and a
  manual rescan; no automatic destructive recovery.
- Add fault-injection coverage of journal persistence failures after publishing.
- Pin GitHub checkout and artifact actions to verified immutable release SHAs,
  disable checkout credential persistence, and enforce `--locked` CI builds.
- Add a bounded in-memory copy queue: 32 waiting jobs maximum, stable operation
  IDs, FIFO dispatch after each finish, read-only inspection and removing jobs
  before they run. Show the active transfer in every theme, even without a
  status bar; pause further copies on error until manually resumed; prevent
  clearing a newer cut selection when an older move ends. Retain 64 recent
  operation results for the current session, including cancellation/failure.
- Validate source identity and basic metadata before publishing staged copies,
  refusing a file replaced or modified while being copied. Do not follow a
  last-moment symlink replacement when opening a regular source file. Preserve
  regular file and folder modification/access timestamps where supported.
  Add regression tests for in-place mutation, path replacement and timestamps.
- Keep operation feedback visible in rice shells that hide the bottom status
  bar; the full message remains available on hover.
- Explain exactly which item stopped a multi-file Copy/Move and how many
  earlier items had already completed. Add a partial-batch regression test;
  recheck the original source identity after cross-device publication before
  beginning source cleanup.
- Add a confirmed batch-rename dialog with a complete prefix/suffix preview
  for up to 256 selected items. Refuse existing or duplicate target paths
  before renaming, never overwrite, retain non-UTF-8 source bytes and report
  partial completion if a competing writer creates a later target. Extend
  item-by-item failure reporting to Trash, Delete and Link actions.
- Extend batch rename with literal search/replace and zero-padded sequence
  numbers. Reject unchanged, invalid or duplicate name previews and refuse
  text replacement of non-UTF-8 names rather than corrupting filename bytes.
  Distinguish strict permanent deletion (missing input is an error) from
  idempotent cleanup of internal staging files, with regression tests.
- Persist a bounded, opt-out tab session with up to 32 absolute locations and
  the active tab; update it on navigation and tab changes. Explicit command-line
  paths take precedence. Restore without synchronous mount checks; skip
  non-UTF-8 names when serializing TOML, and do not replay file operations.
  Expose the setting in the shared Session menu for every rice.
- Keep failure-injection journals in per-test temporary directories and isolate
  the Xvfb smoke test's XDG config/state/cache. This prevents a fabricated
  unfinished-move warning in GUI screenshots and protects a developer's real
  state when running tests locally.
- Bound image previews by the opened target's size and decoded dimensions,
  including symlinks; read image bytes from one nonblocking descriptor with a
  32 MiB cap to avoid reopening a path substituted with a FIFO/device. Reject
  special files before preview and test a real PNG, a large linked target and
  a Unix socket. Keep small source images at their original size rather than
  upscaling them to the preview pane's maximum bounds.
- Use the committed Cargo lockfile for the local installer just as in CI.

**Validation:** The GitHub workflow runs formatting, Clippy, unit tests,
release build, CLI and seven-theme Xvfb smoke for each pushed commit. Consult
the specific commit's workflow outcome before treating an increment as tested.
Real Linux Mint acceptance is still pending.
These changes are not a v1.0 release.

## 0.2.0

- Local Rust/egui file explorer with seven visual profiles.
- Tabs, folder navigation, selection, file operations, worker scans,
  basic properties and image/text previews.
- Cross-device moves with content verification and initial Linux CI.

The v0.2 baseline still has documented functional and safety gaps; consult
[ROADMAP.md](ROADMAP.md) for the current scope and acceptance criteria.
