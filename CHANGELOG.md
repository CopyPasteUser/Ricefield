# Changelog

## Unreleased — v0.2 stability and v0.3 safety groundwork

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
