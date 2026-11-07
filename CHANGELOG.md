# Changelog

## 1.0.0-rc.19 — Owner test protocol prepared

- Bump the development candidate from `rc.18` to `rc.19` after the durable
  WebDAV journal and actual-process crash evidence increments. Add a detailed
  owner acceptance checklist covering Mint, NAS, USB, seven rices, file
  integrity, network collisions, recovery, packaging and low-memory operation.
- Keep stable `v1.0.0` intentionally untagged. Unverified hardware scenarios,
  unsupported backend-specific writes and documented code scope remain open.

## Unreleased — Actual-process WebDAV crash regression

- Extend the independent mock WebDAV server with an opt-in stalled stage-PUT
  response. Kill an actual uploader test process while the server retains the
  private stage and before the HTTP acknowledgement arrives. Assert that the
  local journal survives intact, the original source is untouched and no
  final destination has appeared. The mock remains isolated from real state.
- Document the per-backend remote-write capability matrix, distinguishing
  mock-proof behavior from still-required NAS/hardware sign-off.

## Unreleased — Durable uncertain WebDAV upload evidence

- Before any remote upload stage PUT, synchronously publish a private bounded
  WebDAV journal with the random stage URI and final destination (no login
  secrets). Refuse the upload if evidence cannot be durably saved.
- Retire the local evidence only after the server acknowledges the exclusive
  final MOVE. Recover ambiguous/crashed attempts on the normal background
  recovery worker, with copyable paths and an explicitly confirmed local-only
  journal dismissal after the owner checks the remote server. Never
  automatically retry, overwrite or delete a remote object.
- Isolate mocked WebDAV journal tests from real user state, including failure
  retention and a fail-closed unwritable-journal regression.

## Unreleased — Bounded directory restart recovery

- Allow explicit, two-step confirmation to publish a verified, still-private
  directory stage on Linux with atomic no-overwrite semantics. Compare nested
  bytes, metadata, xattrs and hardlink topology within strict bounds before
  and after publication. Retain the original and reject any uncertainty.
- Permit the same bounded verification when retiring an already-published
  directory journal after a crash. No automatic replay, source deletion,
  unrestricted tree recovery or power-loss sign-off is implied.

## Unreleased — Restart-stage recovery and WebDAV diagnostics

- Explicitly publish a fully verified, unchanged staged regular file or symlink
  on Linux after two confirmations, with a pinned private stage, atomic
  no-overwrite rename, a directory durability barrier and final byte comparison.
  Retain the source and refuse stale, unverifiable or colliding paths.
- On an uncertain WebDAV upload failure, display the exact private staging URI
  for manual server inspection. Failed server safety probes likewise report
  the random prefix of potentially remaining probe objects. Never silently
  delete or retry an ambiguous upload; keep unsupported network writes disabled.
- Register the launcher under the standard `FileTools` desktop category as
  well as `FileManager`, avoiding desktop-file validation hints.

## Unreleased — Explicit verified-publication journal resolution

- After a process crash, let the owner explicitly resolve an already-published
  regular-file/symlink cross-device copy (including the atomic-rename-to-journal window), provided
  recorded inode evidence and a second bounded byte comparison still agree.
  The cancellable two-step GUI action retires only the private recovery record and never
  deletes, overwrites or replays user files.
- Serialize journal mutations with directory locks and reject stale writer
  snapshots. Add disposable recovery regressions for published and half-
  committed records, filename collisions, changed content, cancellation,
  nested directory refusal and stale writers.

## Unreleased — Safe UTF-16 network preview parity

- Extend the bounded read-only GIO prefix preview to BOM-marked UTF-16LE/BE;
  permit an unfinished last code unit/surrogate only when the prefix reader
  reports truncation. Malformed complete files and control characters still
  display the safe hex view. Add regression coverage for both byte orders,
  truncated units and unsafe bytes.

## Unreleased — More resilient local file previews

- Detect PNG/JPEG/GIF/WebP/BMP/ICO from a fixed-size signature on the existing
  open descriptor, so image files without recognized suffixes still get the
  same 32 MiB / 16 MP bounded preview.
- Render BOM-marked UTF-16LE/BE as readable text; invalid sequences and unsafe
  control characters fall back to the read-only bounded hex view. Include
  regressions for extensionless images, both byte orders and control bytes.

## Unreleased — Production crash evidence for cross-device copies

- Record real cross-device Move copies in the durable private move journal,
  including staging, verification and publication transitions. A completed
  copy keeps its source and retires the journal; collisions or interruption
  leave evidence for read-only inspection after restart.
- Fail closed if the journal cannot be reserved. Add isolated success/collision
  tests without enabling automatic rollback, replay or unsafe source removal.

## Unreleased — Nonblocking custom-theme startup

- Display a bundled fallback immediately and load custom TOML themes on the
  existing background worker, so an offline mount cannot stall UI startup.
  In-flight results from a superseded theme are still discarded. Validated
  custom themes replace the fallback when loaded; slow device acceptance is
  still pending.

## Unreleased — Persistent, bounded per-file transfer diagnostics

- Preserve up to 24 source/destination/outcome/error summaries for each recent
  local file operation across restarts, prioritizing failures and unfinished
  entries. Earlier summary-only history remains readable.
- Keep history strictly display-only: restored records can never authorize
  Undo, replay or filesystem mutations. Cap each item and the complete private
  state file; automatically discard oldest records once the 256 KiB limit is
  approached. Full crash recovery and untruncated logs remain out of scope.

## Unreleased — Previews, network tabs, Open With and file icons

- Offer bounded BMP/ICO previews and readable fallbacks when optional PDF,
  media or archive decoders cannot process a file. Unknown binary formats
  continue to show their safe hexadecimal prefix.
- Open remote folders in a new tab from the network toolbar, row context menu
  or discovery browser without replacing the current location.
- Expose **Open With** from local file context menus and previews, and include
  recommended desktop MIME handlers in the existing safe chooser.
- Draw small original vector file-type pictograms across all seven rices;
  recognize common document, spreadsheet and presentation extensions without
  borrowing proprietary application icons.

## Unreleased — Complete RiceField naming

- Rename the Cargo package, binary, desktop entry, icon assets, user-local
  paths, installation locks, CI artifacts, release bundles, tests and docs to
  `ricefield`/RiceField. Prior release-candidate installations may retain an
  obsolete launcher and separate settings directory; back up user settings
  before upgrading and remove outdated desktop shortcuts manually.

## Unreleased — Bounded remote text/binary prefix preview

- Preview the beginning of remote files through a read-only GIO child process;
  output over 32 KiB is truncated intentionally instead of rejected or
  downloaded in full. The reader terminates its producer as soon as the
  prefix is available; cancellation and a 12-second deadline remain active.
- Render binary or invalid UTF-8 data as the same safe 256-byte hex/ASCII
  preview used locally. Avoid misclassifying a text character cut at the
  prefix boundary; label incomplete text and unknown binary sizes explicitly.
- Add tests for large streams, control bytes and UTF-8 boundary truncation.
  NAS/credential/disconnect acceptance remains a real-device gate.

## Unreleased — Interrupted network-download inspection

- Add an explicit, background-only **Inspect interrupted downloads** control
  to remote tabs. It finds up to 64 privately owned staging directories in
  the selected local download folder, without opening their contents or
  removing them. Symlinks, ordinary files and non-private directories are
  excluded; large folders fail with a visible bounded-scan error.
- Let users open a matching folder for manual examination. Other RiceField
  instances may still be using it, so the result never triggers automatic
  cleanup, resume or source deletion. A disposable crash-remnant regression
  checks that partial bytes survive the inspection unchanged.

## Unreleased — ChromeOS Files shell

- Give ChromeOS a distinct Files-like app toolbar, touch-friendly view pills,
  a real contextual overflow menu and a minimal search/address row instead
  of the generic multi-line editor toolbar.
- Add original monochrome navigation symbols and blue rounded folder/file
  artwork, My files and actual local storage locations, real shortcuts,
  network discovery, and mounted volumes with existing safety actions.
- Keep the full advanced file-manager toolbox accessible on demand. Preserve
  read-only remote locations, keyboard shortcuts, user TOML composition,
  native window decorations and shared filesystem operations.
- Native GUI smoke passed with clicked More/grid views, split, compact and
  narrow screenshots (CI run `42339732861`). Physical scaling, NAS and USB
  acceptance remain explicit gates.

## Unreleased — Windows 98 Classic Explorer shell

- Give the classic rice real Explorer-era gray menu/toolbar bands, original
  bevel-like icon buttons, history/address/Go/search controls and functional
  File/Edit/View/Go/Favorites/Tools/Help menus.
- Replace generic Places/Favorites/Volumes card-like buttons with a tree-style
  folder/drive/network rail and tight file list. Paint original 1990s-inspired
  yellow folders and white paper file icons without any Microsoft artwork.
- Hide the modern toolbox by default, retain it via Tools, and route every
  action through shared filesystem and remote write safety controllers.
- Native Xvfb GUI smoke passes for the full shell, clicked Tools menu, split,
  compact and narrow states (CI `42340656430`); published captures are in
  `docs/screenshots/`. Compare all 21 pairs of the seven default rice images
  to detect accidental convergence. Correct Win7's idle transfer band and the
  GUI filter harness coordinates after its removal.
- Physical Linux Mint/NAS acceptance remains a separate gate; window
  decorations stay with the desktop window manager.

## Unreleased — Bounded local binary preview

- Show a read-only hex/printable-ASCII dump of the first 256 bytes of unknown
  binary files in the existing monospace Quick Look / Preview UI. Invalid
  UTF-8 and control characters render as byte values, never terminal escape
  sequences or replacement text. No new dependency or external decoder.
- Test oversized data, non-UTF-8 bytes, control sequences and unchanged file
  contents. Network URI binary preview remains a separate unimplemented gate.

## Unreleased — macOS Finder-inspired shell

- Replace the generic macOS skin with a distinctive Finder-like chrome,
  compact back/forward navigation, location/search row, interactive icon/list
  toolbar, Quick Look, New Folder and a categorized More menu.
- Present local Favorites and actual Locations in a compact illustrated rail;
  retain real bookmarks, mounted volumes, network discovery and SMB/SFTP
  connection rather than imitating unavailable Apple-only services.
- Render original blue folder/document vectors, aligned sortable details
  columns and a responsive two-row toolbar in narrow windows. Keep advanced
  tools on demand so the file surface receives most of the space.
- Preserve shared file-operation safety, read-only remote locations, TOML
  composition, keyboard controls and desktop-owned window decorations.
  The native GUI smoke passes with real screenshots of full, icon, More, split,
  compact and narrow states (CI `42295370012`). Physical Linux Mint and
  hardware/NAS acceptance remain separate verification gates.

## Unreleased — Frutiger Aero · Blue Skies Aqua shell

- Rebuild the former generic cyan Aqua skin into an original sky/water-themed
  file manager with a 48-point horizon masthead, pearl-green navigation rail,
  glass-shelf action dock, readable breadcrumb/search controls and sunlight-
  layered vector folder/document glyphs. Runtime colors and density remain
  customizable through `themes/frutiger-aero.toml`.
- Make the responsive dock's New folder, Pin, Copy, Cut, Paste, Rename, Trash,
  Preview, View, Split, Tools, Network, Refresh and Transfers actions invoke
  real shared controllers. Selected/disabled states follow current location;
  remote locations cannot trigger local writes. Keep advanced tools reachable
  but hidden by default and avoid the idle transfer strip.
- Preserve keyboard shortcuts, file safety, TOML shell composition and
  RiceBowl/window-manager ownership of window decorations. Record real GUI
  screenshots for full, Tools interaction, split, compact and narrow layouts;
  physical Linux Mint and NAS acceptance still need to be performed.

## Unreleased — Windows 10 Explorer ribbon shell

- Replace the generic Windows 10 HOME strip with functional File, Home,
  Share and View ribbon pages. Context-sensitive commands handle paste,
  selection, new files/folders, rename, delete confirmations, properties,
  compressing, network discovery and switching file layouts.
- Add original flat vector ribbon/file/sidebar icons, Quick access, This PC,
  network/device navigation, breadcrumbs and a slim status-view switcher. The ribbon
  wraps into quick commands at smaller widths instead of clipping controls.
- Ensure the File tab has visible blue contrast and remove the idle transfer
  strip from this shell (active transfers and history remain accessible).
- Keep keyboard and file safety logic shared across all seven themes; remote
  tabs cannot trigger destructive local actions. Keep window borders and
  decorations in the desktop/window manager, not RiceField.
- Publish six actual, unedited GUI-smoke screenshots in the README/gallery:
  full, split, compact, narrow, View and Share. Physical Linux Mint acceptance
  remains open like other historical themes.

## Unreleased — Windows 7 Aero Explorer shell

- Give the Aero theme a dedicated navigation row with Linux-safe, clickable
  breadcrumb parents, editable address via Ctrl+L, navigation history, search
  scope options and responsive narrow-window wrapping.
- Replace the generic action strip with functional Organize, Open, Share,
  Favorites, New folder, View and Preview controls. Keep the advanced file,
  archive, filtering and theme tools reachable without cluttering Explorer.
- Bring the sidebar, details-view headers, file-cell metadata, original vector
  file/folder artwork and selection details into a coherent Windows 7-inspired
  layout. Keep decorations owned by the window manager / RiceBowl.
- Preserve TOML recomposition of the Aero shell and custom sidebar append
  compatibility; test real clickable breadcrumb destinations and default
  section ordering. Visual acceptance on physical Linux remains pending.
- Refresh the README hero and screenshot gallery with unedited Windows 7 Aero
  GUI smoke captures (full window, split view, compact and narrow layouts).
- Install only required Xvfb/ImageMagick CI packages: omit optional
  Ghostscript/font recommendations that timed out on hosted Ubuntu mirrors.

## 1.0.0-rc.17 — atomic desktop icon upgrades and bounded release setup (verification pending)

- Publish the desktop SVG icon only after a same-directory staging copy is
  complete. Keep the previously installed icon intact during a slow upgrade;
  add a deterministic regression that failed before the change.
- Apply bounded retries and mirror timeouts to the tagged GitHub release
  build, matching the proven checks workflow so transient runner outages
  cannot hang release preparation indefinitely.
- Keep special-file refusal tests active in restricted containers that deny
  Unix-domain sockets: use FIFO fixtures only when socket creation is denied,
  without skipping the safety checks or changing production behavior.
- Bound a second desktop-measurement attempt to the exact transient
  `XOpenDisplayFailed` runner condition, with visible Xvfb logs. Do not retry
  failed GUI assertions or silently accept repeated display failures.
- The installer is still not power-loss transactional across its three
  installed paths; unresolved code and physical acceptance gates still block
  an official stable `v1.0.0` tag.

## 1.0.0-rc.16 — explicit safe USB drive power-off (verification pending)

- Show **Safely power off USB drive** in the unmounted-removable-volume menu
  only when UDisks2 advertises the USB drive's `CanPowerOff` capability.
- Require user confirmation, refresh the raw ObjectManager inventory and
  refuse power-off if any sibling filesystem remains mounted, even if hidden
  from RiceField's sidebar; reject incomplete or non-USB device metadata.
- Delegate final busy-drive checks, cache flush, physical power-off and Polkit
  authorization to UDisks2 Drive.PowerOff; never force unmount other users'
  filesystems. Test eligible, busy, missing-metadata and non-USB fixtures.
- Physical USB/Polkit/power-loss verification on the owner's Mint system is
  still pending; stable v1.0.0 must not be published yet.

## 1.0.0-rc.15 — no-overwrite Linux extended attributes (verification pending)

- Add `user.*` text attribute creation (max 4 KiB) in the file Properties
  dialog. Existing attribute values are never modified or erased.
- Use a no-symlink-follow file descriptor and compare inode metadata against
  the Properties snapshot before the change. Kernel `XATTR_CREATE` prevents
  concurrent writers from silently replacing an attribute.
- Add tests for existing-attribute collisions, invalid namespaces and
  oversized values, symbolic links and changed/replaced selected files.
- Complex ACL edits, privileged attributes, replacement and removal remain
  deliberately unsupported pending robust safety semantics and device tests.

## 1.0.0-rc.14 — guarded POSIX owner/group changes (verification pending)

- Add editable numeric owner UID and group GID fields to the file Properties
  dialog; buttons stay disabled for symlinks, invalid or unchanged values.
- Apply explicitly requested owner/group changes on the inode pinned by an
  O_PATH file descriptor using `fchownat(AT_EMPTY_PATH)`. No sudo, shell,
  automatic privilege escalation or path-based chown is used.
- Compare the snapshot from the Properties dialog before mutation to refuse
  replaced/changed files. Reject reserved `-1` UID/GID sentinels and special
  files. Display ordinary Linux permission errors rather than hiding them.
- Add disposable tests for authorized same-owner edits, invalid identifiers,
  symlink refusal and stale selection. Real Mint/ACL/Polkit acceptance remains.

## 1.0.0-rc.13 — stale Properties selection cannot change another file (verification pending)

- Carry the original file's identity and change metadata from the Properties
  background snapshot into the user's explicit permission-change action.
- Compare that snapshot against the pinned file descriptor immediately before
  chmod, rejecting files that were replaced, renamed or modified while the
  dialog was open. Preserve Linux non-UTF-8 paths and no-follow handling.
- Add regression coverage for an unrelated file replacing the selected path
  before Apply; the replacement mode and old file contents remain unchanged.
- Real Mint acceptance and the other incomplete 1.0 subsystems remain open.

## 1.0.0-rc.12 — race-safe POSIX permission edits (verification pending)

- Close the symlink time-of-check/time-of-use gap in the existing Properties
  permission editor. `chmod` now operates on a file descriptor that has been
  opened with no symlink following, not on the potentially replaced pathname.
- Support changing permissions on unreadable owned files through Linux O_PATH
  and `fchmodat2(AT_EMPTY_PATH)` where the kernel supports it; reject symlinks,
  special files and nonstandard permission bits.
- Add deterministic regression coverage for renaming a selected file and
  replacing its original name with a symlink to an unrelated file after open.
- Hardware/NAS testing and the remaining 1.0 safety scope are still open.

## 1.0.0-rc.11 — native removable-drive unmount/eject (verification pending)

- Associate mounted removable/USB filesystems with their UDisks2 device and
  drive objects, retaining mount paths as raw Unix bytes (including non-UTF-8).
- Prefer native, Polkit-authorized UDisks2 unmount and eject for recognized
  external devices; keep existing bounded GIO actions for other mounts.
- Before a device mutation, re-query live device state, reject stale or
  missing mount paths, and never force unmount or silently power off hardware.
- Include disconnected/unmounted eject actions only for drives reporting
  Ejectable. Add hostile D-Bus path and mount-point decoding tests.
- Real Linux Mint hardware and Polkit acceptance remain pending; this is not
  permission to enable remote uploads or publish a stable 1.0 release.

## 1.0.0-rc.10 — UDisks2 removable-volume discovery and mount (verification pending)

- Discover unmounted removable/USB filesystems through the Linux UDisks2
  ObjectManager on a background worker, throttled to avoid continuous D-Bus
  enumeration. Missing daemons leave existing mounts and network browsing intact.
- Offer an explicit Mount action in the Volumes sidebar, using the device's
  validated UDisks2 object path and system Polkit authorization (no sudo, shell,
  automatic mounting or forced unmount).
- Filter ignored, non-filesystem and already mounted volumes; handle incomplete
  device metadata conservatively. Add fake D-Bus inventory and hostile-path tests.
- Real Mint USB hardware/Polkit acceptance is still pending; encrypted-volume
  unlocking and remote writes are **not** unlocked by this change.

## 1.0.0-rc.9 — guarded session-only batch rename Undo (verification pending)

- Offer **Undo batch rename** in Transfers after a completely successful batch,
  including swaps and rename cycles. Reversal uses the same atomic Linux
  exchanges and no-overwrite moves as the original operation.
- Preserve each renamed entry's post-operation identity/metadata snapshot;
  refuse Undo entirely if any item was edited, replaced or missing or if an
  unrelated writer now owns an original filename. Guard rollback of ordinary
  execution errors, and report partial states honestly.
- Keep Undo session-only and unavailable for incomplete batches or restored
  read-only history. Do not claim crash-safe batch replay or recovery.


## 1.0.0-rc.8 — atomic batch rename cycles (verification pending)

- Allow safe batch renames whose destinations are other selected filenames,
  including two-name swaps, three-name cycles and chains, while rejecting
  collisions with unrelated files and symlinks before mutation.
- Use Linux `RENAME_EXCHANGE` without temporary names for cycles; use atomic
  no-overwrite moves for empty targets. A crash cannot strand a staged name.
- Attempt reverse-order guarded rollback after ordinary failure/cancellation;
  stop and report partial positions if an external writer changes an entry.
- Restore per-item batch rename results in the Transfers dialog. Add regression
  coverage for cycles, source identity, selected hardlinks and rollback.

## 1.0.0-rc.7 — durable read-only transfer history (verification pending)

- Preserve up to 64 completed local transfer summaries across restarts in a
  private versioned XDG state file, with outcome, elapsed time and item counts.
- Serialize writes through a background worker; reject symlink/special files,
  size-limit reads, atomically publish and sync updates, and prevent competing
  writer processes from overwriting one another's history.
- Keep corrupt or unsupported history unchanged. Restored records cannot
  trigger Undo, replay, deletion or recovery mutations. Add focused tests for
  persistence, corrupted input, interprocess locking and UI conversion.

## 1.0.0-rc.6 — bounded network download queue (verification pending)

- Queue up to 16 explicitly selected remote file or folder downloads, capture
  each local destination at selection time and run one transfer at a time.
- Show pending transfers; allow removing a waiting job, cancelling the active
  job, pausing the queue after the current transfer and manually resuming.
  Pause automatically after a failed or cancelled transfer.
- Reject duplicated pending downloads to the same local target and preserve
  the session-only/no-replay guarantee. Add FIFO, capacity and duplicate tests.

## 1.0.0-rc.5 — regex batch rename preview (verification pending)

- Harden GIO remote listings by distinguishing regular files, directories and
  special entries. Refuse GIO-reported symlink/special-file downloads and
  previews, and fail closed before recursively following a reported link.
  Test cancellation, conflicting destinations and unsupported entry types.
- Add an opt-in regular-expression mode for 2–256 item batch rename previews,
  including capture substitutions with no additional dependencies.
- Bound regex length and compiled program size, validate every generated
  filename and reject non-UTF-8 names for pattern matching; never overwrite
  existing targets. Safe rename cycles and batch Undo remain unimplemented.
- Add regression tests for named captures, invalid patterns and filenames.

## 1.0.0-rc.4 — bounded remote folder downloads (verification pending)

- Add a recursive read-only network-folder download with private local staging,
  cumulative progress, cancellation and atomic no-overwrite publication.
- Enforce 4,096 entries, 512 folders, 16 levels, 8 GiB and 20 minutes for an
  entire download. Reject unsafe child URIs/names and remove ordinary failed
  staging data. Do not expose partial trees as completed folders.
- Add deterministic tree, failure and over-budget regression tests. Remote
  metadata preservation and native network writes are still unsupported;
  hardware/NAS tests remain separate release gates.

## 1.0.0-rc.3 — readable NAS names and destination selection (CI verified; device acceptance pending)

- Show decoded Unicode and spaced filenames on remote shares while keeping
  the original percent-encoded URIs as action identifiers. Malformed escapes,
  separators and control characters retain their escaped representation.
- Allow an explicit local target folder for downloads, with absolute-path
  validation and persistence in the existing backwards-compatible config.
- Align visible window/launcher/CLI branding to RiceField; preserve the
  `ricefield` command, desktop file identifiers and settings locations.
  Update the graphical and packaging tests for the visible window name.
- Keep remote writes and full-folder downloads disabled pending safety work;
  this is not yet a stable release.

## 1.0.0-rc.2 — explicit safe remote downloads (verification pending)

- Add an opt-in download action beside remote files, streaming via GIO/GVfs
  to the local Downloads directory without blocking the graphical thread.
- Bound each download to 8 GiB and 20 minutes. Show transferred bytes, allow
  cancellation, and stage privately before an atomic Linux no-overwrite
  rename. Existing files and symlinks are never replaced by a download.
- Test encoded name/path traversal rejection, successful streaming, process
  failure, duplicate destination and cancellation. Full NAS/offline and Mint
  acceptance remains open; no remote uploads or folder downloads are claimed.
- Correct the Cargo repository URL after the RiceField GitHub rename and
  update the current user and release documentation.

## Documentation — a shorter entry point

- Replace the accumulated release-by-release README with installation,
  everyday capabilities, first steps and the current support boundary.
- Move detailed workflows, keyboard reference and theme composition into a
  user guide; collect launcher, clipboard, NAS, mount and recovery diagnosis
  in troubleshooting. Keep history in this changelog and remaining work in
  the roadmap/status ledger.
- Include the linked Markdown guides and theme examples in release bundles
  so the documentation remains navigable after extraction.

## 1.0.0-rc.1 — local release candidate, hardware acceptance open

- Preserve press-time keyboard modifiers when a complete Ctrl/Alt chord
  arrives between frames; dispatch ordered key events and stop when one opens
  a dialog. Regression-test coalesced press/release rather than masking missed
  shortcuts with slower GUI test input.
- Use readable `asc` / `desc` sort-header text instead of missing arrow glyphs.
- Keep the Filters popup open while expanding options and editing fields;
  the framework's default close-on-every-click made custom inputs unusable.
  Exercise invalid-regex correction and persisted edits through the native GUI.
- Pin development CI to Ubuntu 24.04 as well as release CI so its desktop
  artifacts target the Linux Mint 22 generation rather than a moving runner.
- Resolve CLI/address file paths and symlinked folders in scan workers;
  reuse cached entry types for activation and preview actions. Refresh sidebar
  place availability with volume snapshots instead of probing on every frame.
- Add opt-in native FileManager1 `ShowFolders`, local `ShowItems` and local
  `ShowItemProperties`. Bound requests, reject unsafe URIs, preserve existing
  bus ownership, and defer requests behind confirmations. Test real protocol
  calls and owner contention on a private bus plus visible GUI properties.
- Add bounded Unicode filename regex filtering, persisted with existing
  filters and covered for invalid input and adversarial patterns.
- Add an explicit disposable `--benchmark`: 100,000 real directory entries,
  natural sorting, glob filtering, bounded search and a byte-verified 64 MiB
  copy. Add Xvfb mapped-window startup and ten-second idle CPU/RSS measurements.
  Both CI pipelines retain performance reports and desktop integration images.
- Document the exact supported candidate scope, verification commands and
  remaining release gates in `docs/1.0-STATUS.md`. This candidate does not claim
  native remote mutations, automatic crash rollback, portals, RiceBowl IPC,
  complete German localization or physical Linux Mint acceptance.

## 1.0 CI resilience — bounded graphical smoke runtime

- Consolidate previously separate graphics and Xvfb `apt` installations into
  one dependency step. Retrospective runner logs attributed the apparent GUI
  stall to an eight-minute second package install; the actual GUI script
  completed in about one minute.
- Fail with application/window diagnostics when a headless X11 focus request
  cannot complete within eight seconds, rather than hanging indefinitely.
- Bound the entire eight-profile Xvfb smoke in CI and the release workflow
  to eight minutes with a short forced-cleanup grace period. Full checks are
  still mandatory; timeouts are failures, never assumed passing results.

## 1.0 CI efficiency — verified pinned Cargo cache

- Cache Rust registry, Git dependencies and incremental `target` outputs in
  the development and release pipelines using a SHA-pinned official GitHub
  cache action. Distinguish runner image and architecture in cache keys;
  invalidate dependency snapshots when `Cargo.lock` changes.
- All format, Clippy, test, release and graphical gates still execute on
  every run; a cache hit is only an accelerator, never proof of correctness.

## 0.9 interaction safety — shortcut focus isolation

- Suspend global file and tab keyboard shortcuts behind open network login,
  mount-confirmation, discovery and recovery windows, while preserving normal
  Ctrl+N behavior after focusing the address/search fields.

## 0.9 navigation accessibility — reliable folder tree controls

- Replace missing-glyph Unicode disclosure triangles with visible native
  `+`/`-` tree expand/collapse buttons across every rice, retaining tooltips,
  focus and keyboard activation. Align the UX contract with read-only URI tabs.

## 1.0 release safety — serialize user install and uninstall

- Protect install, upgrade, rollback and uninstall with a common per-user
  `flock` lock in private XDG state, preventing a failed upgrade from restoring
  stale backups over a concurrently updated executable.
- Test the held-lock behavior for both installers and uninstallers, reject a
  substituted symlinked lock or relative state root, and bundle the lock
  helper with binary releases. The lock file intentionally survives uninstall.

## Unreleased — 0.9.0-alpha.6 Linux URI launch interoperability

- Accept supported SMB/SFTP/WebDAV/MTP URIs as explicit CLI locations and
  open them as read-only GIO tabs, alongside native paths and local file URIs.
- Register the desktop launcher with `%U` so Linux desktop open requests can
  pass a URI to the same validated location parser. Keep passwords out of
  command lines; no system default file-manager association is claimed.

## 1.0 release preparation — atomic user install and rollback

- Publish installed executables with an atomic same-directory rename to avoid
  overwriting a running binary in place. Restore the preceding binary, desktop
  launcher and icon when registration fails during an ordinary update.
- Reject symlink/non-file installation targets and preserve user data. Test
  clean installs, upgrades, injected failures and rollback in isolated homes.
- Teach bundle packaging and GitHub release automation to accept future
  `1.0.x` tags. No stable 1.0 tag has been created or signed off.

## 0.9 preview stress coverage — 100k virtual file entries

- Add a deterministic 100,000-name natural-sort/glob regression test with no
  fixture writes or extra dependencies. Real startup, idle RAM, HDD scans and
  interactive frame-time measurements remain mandatory for 1.0 acceptance.

## 0.9 preview CI hardening — distinct shell regression gates

- Assert distinct Win98/Win7/macOS toolbar structures and Navigation presence.
- Exercise a structurally custom top/bottom shell in the Xvfb GUI matrix,
  and fail CI if 98/7/macOS (or the custom shell) become pixel-identical.
- Screenshots remain an automated regression signal, not owner visual sign-off.

## Unreleased — 0.9.0-alpha.5 shell component composer

- Build actual top/bottom toolbar composition from the visual theme editor:
  choose shell kind, show/hide/reorder components, move Navigation between
  regions without ever removing it, and preserve the layout in TOML.
- Configure sidebar and preview placement, sidebar sections and collapsing,
  and status-bar visibility without touching file-operation code.
- Synchronize visual sidebar-width changes with the active shell override;
  validate and roundtrip composed structures. Real visual acceptance remains
  scheduled for after the code-complete 1.0 candidate.

## Unreleased — 0.9.0-alpha.4 bounded remote filename search

- Enable case-insensitive filename search in SMB/SFTP/WebDAV/MTP tabs using
  explicitly requested, read-only GIO listings; Subfolders enables BFS search.
- Bound traversal to 64 folders, 16,384 inspected entries, 2,048 matches,
  depth 8 and 12 seconds; allow cancellation, skip failing subfolders and
  visibly label incomplete results. Never scan remote file contents.
- Query changes debounce and cancel obsolete workers; selected remote tabs
  remain read-only. Actual Mint/NAS behavioral acceptance is deferred to 1.0.

## Unreleased — 0.9.0-alpha.3 private visual theme editor

- Edit validated TOML in-app or visually modify palette colors, icon size,
  row height and sidebar width. Import an absolute local TOML file without
  mutating it, with validation and an informative error state.
- Explicit Save & use writes a *new* private theme file with restricted name,
  256 KiB size cap, no-overwrite creation and 0600 file permissions. Remember
  editor-owned themes in configuration and restore them at startup, while CLI
  theme overrides continue to work. Bundled themes remain immutable.
- Full RiceBowl theme event synchronization and real Mint acceptance remain open.

## Unreleased — 0.9.0-alpha.2 native MIME Open With

- Discover local file MIME type and registered application desktop IDs via
  bounded cancellable GIO workers, then offer them in a common native dialog.
- Launch the chosen registered desktop entry with `gio launch` and a separate
  path argument; never execute desktop Exec strings or pass input to a shell.
- Reject malformed MIME/desktop IDs, report absence of desktop handlers,
  avoid blocking UI while probing. Full portals/FileManager1/Mint acceptance
  remains open; this is not a stable 0.9 release.

## Unreleased — 0.9.0-alpha.1 live external rice TOML

- Watch an explicitly loaded `--theme-file` at the existing three-second UI
  idle cadence; apply valid edits live without restarting or interrupting tabs.
- Reject invalid/oversized/non-regular TOML inputs and retain the last good
  theme, with visible error feedback and a manual Reload control. Choosing a
  bundled rice disables external watching for that window.
- Cap theme file reads to 256 KiB and avoid opening potentially blocking FIFO
  files on Unix. Direct RiceBowl event integration, TOML visual editing,
  hardware checks and official 1.0 acceptance remain open.

## Unreleased — 0.8.0-alpha.5 read-only duplicate candidates

- Add a background, cancellable duplicate-candidate viewer for current visible
  local files: group by exact size, compare SHA-256, display grouped paths.
- Refuse special files and symlinks (including no-follow opens), bound scanning
  to 256 hashes, 128 MiB per file, 512 MiB in total and 45 seconds before
  starting each hash. Partial/unreadable results are explicitly marked.
- No deletion or automatic cleanup is associated with this view. Hash results
  are advisory because files may change after inspection. Actual disk/USB
  acceptance is still required after the 1.0 code-complete milestone.

## Unreleased — 0.8.0-alpha.4 saved local searches and private settings

- Add an opt-in Saved searches menu (up to 16 validated local folder/query/
  scope combinations); ordinary search text stays transient and no search
  executes at startup.
- Write preferences through a private, unpredictable, create-new staging file
  with atomic replacement. The RiceField-specific config directory is 0700
  and file mode 0600 on Unix; legacy fixed `.tmp` paths are untouched.
- Compile Unicode filename globs to bounded, bit-parallel matchers that need
  constant work per input character even for pathological wildcard patterns.
- Regression tests cover Unicode wildcard semantics, local-only saved-search
  validation and private config replacement. No stable release sign-off yet.

## Unreleased — 0.8.0-alpha.3 custom local filters

- Add case-insensitive filename globs (`*`, `?`) and optional numeric size
  (MiB) and modified-age (days) bounds, combined with basic type/size/date
  presets in one collapsible Filters menu.
- Persist only the explicitly configured filter policy, never search content;
  count all active constraints and reset them in one action. Preserve selection
  safety when custom filters hide files. No extra dependencies.
- Add Unicode/pattern/boundary tests; 100k-folder latency and Mint acceptance
  are still outstanding. This is not a stable release.

## Unreleased — 0.8.0-alpha.2 bounded local content search

- Add opt-in **Contents** search for small UTF-8 local files, combined with
  filename matching and the independent **Subfolders** scope control.
- Run scans on cancellable background workers with limits of 2 MiB per file,
  4,096 inspected files, 64 MiB of total reads and 12 seconds per search;
  report when results are incomplete. Skip symlinks, special files and
  cross-filesystem recursion, with no indexer or stored search text.
- Keep selected items in sync with content matches and share the same search
  controls across all rice profiles. Physical and network acceptance is still
  deferred; this is an alpha, not a stable 0.8 sign-off.

## Unreleased — 0.8.0-alpha.1 local search filters

- Combine type, file-size and modification-date presets in one compact Filters
  menu with a visible active count, persistent defaults and a Clear action.
- Drop invisible paths from the active selection after filtering or search,
  so destructive commands cannot accidentally target hidden selections.
- Keep filters in-memory and theme-independent; content/regex and network
  search remain open. Previous 0.6/0.7 Mint/NAS/hardware gates are not waived.

## Unreleased — 0.7.0-alpha.1 previews and metadata

- Add a 96-entry LRU image thumbnail cache with a 48-item priority queue,
  single background decoder, file-size/pixel limits, negative results for
  corrupt images, and lazy requests for visible grid cells only.
- Add Space-to-toggle resizable Quick Look independent of the side preview,
  without granting any file modifications. Preview work is single-flight with
  latest-request-wins scheduling.
- Extend read-only preview to first-page PDF (`pdftoppm` optional), media
  metadata (`ffprobe` optional) and bounded archive entry listings. Image,
  text and code previews retain input limits and reject special files.
- Expose extended-attribute names and Linux POSIX ACL presence in Properties,
  plus on-demand bounded folder size or SHA-256 via the opened file descriptor.
  Deep metadata is cancellable, asynchronous, and never writes to files.
- Add regressions for corrupt/control-character input, over-limit files,
  thumbnail queue ordering, symlinks, special files, malformed images and
  bounded folder traversal.
- **Not a stable release:** malformed-file fuzzing, Mint image/PDF/media
  backends, hardware performance and unresolved 0.6 NAS acceptance remain open.

## Unreleased — 0.6.0-alpha.8 passive NAS discovery and opt-in keyring

- Add a cancellable, read-only network browser that finds SMB servers from
  GVfs workgroups/network aliases and Avahi's advertised `_smb._tcp` services;
  browse server shares or open their sign-in dialog without typing an address.
- Add opt-in credential saving through the desktop Secret Service via
  `secret-tool` with password supplied on stdin, never argv/config/logs; load
  and forget saved per-URI credentials in the native sign-in dialog.
- Remember a network URI in favorites following successful GIO connect. This
  persists only its location, not the password. Network mutation and automatic
  authentication remain disabled. Real Mint/NAS/keyring acceptance is open.
- Include parser/command safety regression tests; no stable release tag.

## Unreleased — 0.6.0-alpha.7 desktop integration and mount triage

- Install a discoverable application-menu entry in both source and downloaded
  release installs, pointing to the executable's absolute path. Honor
  `XDG_DATA_HOME`, refresh the user menu where supported, and uninstall the
  matching per-user launcher and icon. Cover spaced home paths and packaged
  installation in automated smoke checks.
- Enable reproducible `0.6.x` release bundles, which were incorrectly refused
  by the package version gate, without claiming stable release acceptance.
- Keep `/mnt/medien` as a **failed** physical acceptance case: the Linux
  `read_dir` call returned ENODEV. Do not mask a failed mount with zero items,
  or remount live storage automatically; collect `findmnt`/`ls` evidence first.

## Unreleased — 0.6.0-alpha.6 NAS sign-in and external drag bridge

- Add username, optional workgroup and masked password fields to network
  Connect and Reconnect. Answer GIO's interactive mount prompts with a bounded
  input pipe, not with URI/query credentials or shell arguments; no keyring
  integration or password persistence.
- Add **Drag to another app…** to the local file context menu, launching
  optional `ripdrag` or `dragon` only on demand; existing internal drag stays
  unchanged. Direct tile-to-app drag still requires framework support.
- Pending before stable 0.6: actual Synology and GDBus/GVfs acceptance on Mint,
  X11/Wayland external drag interoperability and real pointer/accessibility tests.

## Unreleased — 0.6.0-alpha.5 shell widgets and compact-dialog checks

- Split file cells (including input response) and sorting headers into the
  read-only shell renderer; dispatch selection, navigation and filesystem
  actions exclusively from the controller. Toolbar buttons now return semantic
  events from a separate, rice-aware presentation module, and sidebar group
  headers and common file dialogs use that same UI layer.
- Make navigation fields stack into three rows in compact windows and constrain
  primary file dialogs to the available width. Preserve all seven existing
  visual identities without requiring new dependencies or changing file
  operations.
- Upgrade the Xvfb regression matrix from crash-only checks to visible split
  and dialog-dismissal assertions, plus 380px-wide screenshots in eight profiles.
- Still blocked for stable 0.6: native outbound drag-and-drop requires source
  support in the Linux window/backend integration, plus real X11/Wayland Mint
  pointer, keyboard, screen-reader and removable-device acceptance.

## Unreleased — 0.6.0-alpha.4 shell renderer and interaction checks

- Extract file-cell painting (list/grid icons, labels, selection/hover and
  theme colors) into `src/shell_renderer.rs`. The app's selection, navigation,
  context-menu and drag/drop code now invokes a presentation-only primitive;
  it cannot run file operations. No visual profile is intentionally changed.
- Extend the native Xvfb smoke across all seven bundled skins and the custom
  sidebar with Ctrl+T/Tab cycling/reordering/closing/reopening, Ctrl+N/Escape
  dialog dismissal and compact 700x470 screenshots. Exercise Ctrl+Shift+N
  second-window creation in the Windows 7 profile with explicit cleanup.
- Retain outstanding native external drag-out and full hardware/accessibility
  acceptance as explicit stable-0.6 blockers. Automated smoke tests do not
  certify real pointer hit targets or network device behavior.

## Unreleased — 0.6.0-alpha.3 mounted-folder resilience

- Do not let a single disconnected FUSE/USB child or transient metadata error
  make the entire parent folder look unreadable. Preserve remaining entries,
  retain a best-effort child entry for retry/navigation, and warn about partial
  metadata with a **Retry listing** action. Still fail visibly when the opened
  directory itself cannot be read; never invent an empty-folder success.
- Include mounts whose actual mount point is `/mnt` (not only `/mnt/*`) in
  the Volumes sidebar. Trust `/proc/self/mountinfo` for discovery even when a
  temporarily disconnected mount cannot be statted.
- Keep usable folder-tree branches visible if a sibling's type lookup fails,
  and identify Linux ENODEV/ENOTCONN/ESTALE folder errors as mount problems.
  Tests inject the ENODEV metadata failure and cover root error propagation.
- A screenshot alone cannot establish whether Linux can still read `/mnt`;
  hardware/FUSE recovery and actual Mint mount acceptance remain open.

## Unreleased — 0.6.0-alpha.2 navigation expansion

- Add a two-pane local split view (F3), with independently focused tab contents,
  selection, scrolling, folder navigation, and one shared action dispatcher.
  Split panes reference stable tab IDs so reordering cannot silently redirect
  operations; closing either pane leaves the remaining tab intact.
- Persist bounded per-tab Back/Forward history (40 entries per direction) and
  the 16 most recently closed tab locations across restart. Validate every
  saved location; never persist search text, credentials or pending operations.
- Support launching a second, standalone local Explorer window with
  Ctrl+Shift+N; secondary windows cannot overwrite the primary tab session.
- Add an asynchronously loaded, symlink-safe folder tree, natural numeric
  filename ordering (plus legacy lexical order), file-kind filtering, and
  clickable list sort headers across all seven skins.
- Add internal copy-by-drag between folders/panes and make OS file drops
  target the folder below the pointer. Copies use the existing no-overwrite
  operation queue and block self/descendant targets. Refuse drops on read-only
  remote views; external drag-out is not yet available in egui's native backend.
- Centralize keyboard and sidebar quick actions, preserve responsive
  loading/empty/retry states. Extend eight-profile Xvfb smoke with an F3 split
  toggle and screenshots. Keep real Linux desktop drag/drop and
  multi-window acceptance outstanding before stable 0.6.

## Unreleased — 0.6.0-alpha.1 development preview

- Begin 0.6 everyday navigation: duplicate and reorder tabs through a shared
  command dispatcher; right-click any tab for actions, cycle tabs with
  Ctrl+PageUp/PageDown, move them with Ctrl+Shift+PageUp/PageDown and reopen
  closed tabs with Ctrl+Shift+T (session-only, most recent 16).
- Preserve local/remote location, Back/Forward history and search in duplicated
  and reopened tabs, but allocate fresh scan/preview cancellation tokens;
  tab reorder persists in the existing bounded location-only session snapshot.
  Do not replay pending operations, selections or ephemeral history at startup.
- Test upstream encrypted ZIP/7z/RAR fixtures independently against libarchive
  3.7.4. ZIP traditional and WinZip AES-128/256 full reads pass; additional
  encrypted ZIP extensions and encrypted 7z/RAR vectors fail closed on this
  backend. Document exact results and a reproducible backend probe in
  [docs/ARCHIVE-INTEROPERABILITY.md](docs/ARCHIVE-INTEROPERABILITY.md).
- Maintain owner-approved deferral of keyrings and physical Mint/USB/network
  interruption acceptance. 0.5 stable and 0.6 stable are not signed off.

## Unreleased — 0.5.0-alpha.2 candidate

- Add masked session-only passwords to archive browsing, reload, verification
  and extraction; support AES-256 ZIP creation and rejection of wrong passwords
  without exposing partial output. No keyring or plaintext persistence.
- Add hostile TAR symlink/device, ZIP drive-path, duplicate member, forged
  4 GiB size, truncated stream, wrong password and extraction-cancel tests.
  Libarchive may normalize ZIP backslashes to relative directory separators;
  final paths are checked for escapes and drive syntax.

## 0.5.0-alpha.1 — archive preview baseline

- Start 0.5 archive management with a runtime-loaded libarchive backend.
  Browse and verify local ZIP, tar, tar.gz, tar.xz, tar.zst and 7z archives,
  with optional RAR support determined by installed libarchive; create ZIP,
  tar.gz, tar.xz, tar.zst and 7z. The Archive menu works with all visual
  rice profiles. Integrity reads, compression and selected extraction run in
  a cancellable worker with bounded progress updates.
- Validate every archive entry path before extraction; reject parent/absolute
  traversal, foreign separators, symbolic/hard links, device/special files,
  duplicate paths and excessively large or deep archives. Extract into a
  private stage and publish a new output directory by atomic no-overwrite
  rename; failed/cancelled work leaves no exposed partial extraction.
- Explicitly defer keyring work and physical Mint/USB/network-disconnect
  acceptance at the owner's request. Keep network writes and credential
  persistence disabled. Do not claim 0.4 stable acceptance from this decision.
- Interactive passwords and expanded hostile-corpus fuzzing remained open in
  this baseline; it was not a stable 0.5 release.

## 0.4.0-alpha.1 — merged groundwork, not a stable release

- Add bounded, asynchronous, read-only GIO text previews for files in remote
  tabs, with a 32 KiB cap, 12-second timeout, cancellation and stale-result
  rejection. Reject binary/control data; no remote mutation or download is
  enabled. Virtualize large remote directory lists to avoid UI stalls.
- Unify Back/Forward history across local and network locations, preserving
  exact native local paths. Clear stale tab results on navigation, avoid
  false Home history entries when restoring/duplicating remote tabs, skip
  inactive local fallback paths in periodic remote-tab refresh checks, and
  verify local → remote → local navigation by regression test. The shared
  Show hidden toggle now refreshes remote GIO listings with hidden entries.
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
  Unix RiceField processes with an advisory directory lock. Open that
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
  that RiceField cannot yet copy. Refuse publication or source cleanup
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
  caused by RiceField's own earlier hardlink removals. This narrows but
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
