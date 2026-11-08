# RiceField 1.0 — Owner acceptance checklist

**Status:** prepared for an owner-run test campaign; **not yet signed off**.
Only mark a check after actually observing it on the indicated hardware.
Automated CI results do not count as physical Mint, NAS or USB evidence.
Use the exact candidate commit/version tested; do not silently substitute a
later development build.

The `1.0.0-rc.20` development candidate passed
[GitHub CI #843](https://github.com/CopyPasteUser/Ricefield/actions/runs/42402923975)
with 382 Rust tests plus Linux release, installation and GUI checks. This is
automated evidence only; **all boxes below require owner verification**.

## Preparation and safety

1. Keep Thunar or the current working file manager installed and unchanged.
   Back up RiceField settings and any personal data that could be affected.
2. Use an **empty, disposable** NAS share/directory and a spare USB drive
   containing **no valuable files**. Do not experiment with unsafe remote
   write or power-off actions against the real media library.
3. Test on Linux Mint 22.x with a normal user account, both X11 and Wayland
   **where available**. Record desktop/session, kernel, filesystem type,
   machine RAM, screen resolution, scaling and GPU.
4. For the low-memory check, use an actual 4 GB machine/session if one is
   available. A 16 GB host with unused RAM is not equivalent.
5. In a working source checkout, run the following *read-only/automated*
   verification and retain the final summary:

   ```bash
   git rev-parse HEAD
   cargo fmt --all -- --check
   cargo clippy --locked --all-targets -- -D warnings
   cargo test --locked --all-targets
   cargo build --locked --release
   target/release/ricefield --version
   target/release/ricefield --list-themes
   target/release/ricefield --benchmark
   ```

6. To create **only disposable local fixtures** for GUI tests:

   ```bash
   RICEFIELD_QA_ROOT="$(mktemp -d)"
   mkdir -p "$RICEFIELD_QA_ROOT/source/nested" "$RICEFIELD_QA_ROOT/destination"
   printf 'test bytes\n' > "$RICEFIELD_QA_ROOT/source/hello.txt"
   printf 'more bytes\n' > "$RICEFIELD_QA_ROOT/source/nested/child.txt"
   ln -s hello.txt "$RICEFIELD_QA_ROOT/source/hello-link"
   printf 'must remain unchanged\n' > "$RICEFIELD_QA_ROOT/destination/hello.txt"
   printf 'Fixture: %s\n' "$RICEFIELD_QA_ROOT"
   ```

   Keep the printed fixture path for the entire acceptance session. Clean it
   up through your file manager after reviewing the test results; avoid
   recursive deletion commands against unverified variables.

## A. Installation and integration

- [ ] **A01 Clean install:** `bash install.sh` under a regular user; the
  application starts from a terminal *and* the desktop application menu.
- [ ] **A02 Upgrade:** run `bash install.sh` over the previous RC with saved
  bookmarks, settings, custom themes and session; no settings or user files
  disappear. Check obsolete pre-rename launchers manually.
- [ ] **A03 Uninstall/reinstall:** in a separate disposable account or test
  environment, run the bundled `uninstall.sh`, confirm it removes only
  RiceField's own installed binary/icon/launcher, and reinstall. The system
  default file-manager association must not change without explicit consent.
- [ ] **A04 Package and rollback:** verify the candidate archive's SHA-256,
  exercise installation from the archive, and test a deliberately injected
  installation failure only in a disposable home. Old working app assets
  remain usable after rollback.
- [ ] **A05 Linux integration:** launch a folder, `file://` URI and a file
  from the desktop/menu; FileManager1 service respects any existing owner.

## B. Navigation, keyboard and file operations

- [ ] **B01 Tabs/session:** create, reorder, close, reopen and restore local
  tabs. Verify Back/Forward, breadcrumbs, editable path and recent locations.
- [ ] **B02 Split/tree/views:** switch split panes, folder tree, icon/list
  views, sorting, natural names, hidden files and selection. Both panes keep
  independent context.
- [ ] **B03 Local copy/move:** copy then move disposable files within one
  filesystem. Confirm complete bytes and correct destination name.
- [ ] **B04 No-overwrite:** copy `source/hello.txt` onto the already-populated
  `destination/hello.txt`. Existing destination bytes must remain unchanged;
  any Keep Both action must use a distinct safe filename.
- [ ] **B05 Cross-device move:** with two *disposable* filesystems, start a
  Move. The verified destination appears complete; RiceField intentionally
  **keeps the original** for a separate manual cleanup decision.
- [ ] **B06 Cancellation/partial batch:** cancel a multi-file transfer;
  completed items and uncompleted items are identified separately. No
  partially written final name is shown as completed.
- [ ] **B07 Symlinks/hardlinks:** try ordinary relative symlinks and a
  hardlinked pair inside a disposable directory. Verify links and relevant
  metadata survive supported operations; unsafe loops/special files fail
  visibly without silently following target data.
- [ ] **B08 Rename/batch rename:** rename, preview conflicts, apply a batch,
  and use guarded **session-only** Undo. After restart no stale Undo action
  must be offered as durable recovery.
- [ ] **B09 Trash/delete:** trash and restore a disposable file. Confirm
  irreversible deletion is clearly gated, never triggered by a preview.
- [ ] **B10 Properties/metadata:** inspect owner, group, permissions,
  timestamps and extended attributes. Test only authorized non-privileged
  edits; protected changes must fail safely and state why. On a disposable
  ext4 file, create a new POSIX access ACL for a test UID (`r--`), verify via
  `getfacl` and check RiceField's read-only access ACL rights summary.
  On a disposable directory with a default ACL, confirm its named users,
  groups, mask and other entries are displayed. Then try creating another:
  the existing ACL must remain bytewise
  untouched. Try a symlink and a read-only filesystem; neither may silently
  change another inode. Existing ACL replacement is intentionally unavailable.
- [ ] **B11 Error paths:** attempt a read-only directory, inaccessible
  folder, too-long filename, self-copy and disappearing source. Confirm
  actionable errors, no UI hang and no mutation of unrelated files.

## C. Search, previews, archives and desktop helpers

- [ ] **C01 Search:** filename, glob, Unicode regex, content search,
  filters, saved queries and cancellation; incomplete results are labeled.
- [ ] **C02 Previews:** known PNG/JPEG/GIF/WebP/BMP/ICO, UTF-8/UTF-16 text,
  PDF, video/audio where optional decoders are installed, source code and
  unknown binary. Unsupported types must show a bounded readable fallback,
  not a crash or unsafe execution.
- [ ] **C03 MIME Open With:** inspect recommended handlers, choose an app,
  cancel the chooser, and try a non-executable/unknown file. Never silently
  open with an unrelated handler. Also click **Choose another app with system
  dialog…**: on a desktop with OpenURI portal version 3 or newer the native
  chooser should appear without changing default associations; on an older
  desktop it must give a clear error without hanging the RiceField window.
- [ ] **C04 Archives:** list/extract a clean ZIP and tar.gz, create a small
  archive, and test available encrypted ZIP/7z/RAR formats. Unsupported
  encryption must report its backend limitation. Use hostile-path fixtures
  only in isolated disposable folders; no traversal escapes extraction root.
- [ ] **C05 Clipboard/drag helpers:** copy, cut, paste and external desktop
  clipboard exchange. Test drag-out when supported; unavailable portal or
  shell functionality must fail honestly instead of losing selection.

## D. NAS, network tabs and guarded single-file uploads

- [ ] **D01 Discovery/login:** discover/connect to the actual disposable NAS
  share and verify explicit authentication and optional Secret Service save.
  No password appears in command lines, logs or session restore.
- [ ] **D02 SMB/SFTP:** browse nested folders, open a folder in the current
  and in a **new network tab**, navigate Back/Forward, sort/search, preview
  and download a test file without overwriting an existing local target.
- [ ] **D03 MTP:** if an actual phone/handheld is available, test browse,
  explicit download, disconnect and reattach. Unsupported write commands
  must remain unavailable.
- [ ] **D04 WebDAV safe upload:** on an explicitly configured **test** WebDAV
  server, upload a single small file, verify exact remote bytes and preserved
  local source. WebDAV server capability probes must run before publication.
- [ ] **D05 WebDAV collision:** create the same target first; a second upload
  must not change it. Repeat with two RiceField clients targeting the same
  disposable name. Exactly one may publish; neither may overwrite.
- [ ] **D06 Offline/cancel:** interrupt a disposable network transfer during
  a real NAS outage or deliberate cancel. Inspect the remote final/stage and
  local evidence; RiceField must not automatically retry or delete unknown
  server objects.
- [ ] **D07 Recovery after restart:** if an uncertain WebDAV/SFTP/SMB attempt leaves
  a private stage record, restart RiceField. **Review safely…** must show
  stage and destination, copyable independently. Only after *you* inspect
  both server paths should the two-step local-only dismissal be tested.
- [ ] **D08 Unsupported remote writes:** MTP upload and all remote rename and
  delete are **intentionally unavailable**. A WebDAV/SFTP/SMB server lacking
  the required no-replace capability must also refuse the upload.
- [ ] **D09 Failed authentication/TLS:** reject invalid login, offline host,
  untrusted TLS and plaintext HTTP except a deliberately opted-in test
  endpoint. Never leak a password into error messages or URL history.
- [ ] **D10 SMB2/3 signed upload:** on a **disposable** Synology test share
  whose ACLs forbid other users from modifying stages, try a small ASCII-named
  file. Check SMB signing, no-replace hardlink support, full byte equality,
  local source preservation and a pre-existing destination that must remain
  unchanged. Repeat two-client collisions, mid-upload cancel and intentional
  disconnect; inspect stages and journal without automatic replay or cleanup.
  If hardlink capability or share isolation is unavailable, leave writes off.
- [ ] **D11 OpenSSH-SFTP upload:** on a disposable SFTP account with trusted
  host key and SSH key/agent, check private `0700` probe/stage directories,
  exclusive hardlink publication, exact bytes and occupied destination;
  test two-client race, cancellation, offline recovery and local-only evidence
  dismissal after remote inspection.

## E. USB, volumes and removable media

- [ ] **E01 Discovery/mount:** plug in a spare removable drive, detect it,
  mount through UDisks2/Polkit, navigate and copy disposable files.
- [ ] **E02 Unmount/eject:** refuse an unsafe busy operation; show a clear
  explanation. After a successful unmount, file tabs handle the missing path.
- [ ] **E03 PowerOff:** only on a spare drive reporting PowerOff support,
  verify explicit confirmation and refusal while *any* sibling filesystem
  remains mounted. For a disposable LUKS USB drive, verify PowerOff refuses
  when unlocked and permits a normal system-authorized request only after
  successful Lock; missing metadata must fail closed. Reattach and inspect
  data with the OS.
- [ ] **E04 Filesystems:** test at least ext4 and exFAT where available;
  record FAT/NTFS/NFS/SMB behavior separately. Unsupported ACL/xattr behavior
  must be reported, not hidden or emulated unsafely.
- [ ] **E05 Encrypted media:** on a **disposable** LUKS-formatted USB drive,
  verify RiceField detects the locked volume, masks its passphrase input,
  rejects an incorrect password without exposing its text in history/logs,
  and unlocks via UDisks2/Polkit with the correct password. Confirm that the
  decrypted device-mapper filesystem appears as a mountable removable volume,
  can be mounted/browsed, and cannot be locked while busy/mounted. Unmount it,
  then explicitly Lock; re-open and verify data. Cancel the password dialog,
  disconnect the drive mid-operation, deny Polkit and verify clear errors and
  no accidental mutations. Unencrypted and internal fixed drives must never
  acquire an Unlock button. Unit fixtures exist, but **native LUKS hardware
  acceptance has not passed**.

## F. Crash recovery and data integrity

- [ ] **F01 Interrupted local transfer:** use only disposable large files
  and a dedicated RiceField test process. Interrupt after a staged copy
  begins; reopen. Original data must remain and a recovery banner must show
  unfinished evidence without auto-delete/auto-replay.
- [ ] **F02 Recovery review:** compare source/destination or verified stage
  through the UI; stale identity, changed bytes and destination collisions
  must disable unsafe publication/resolution.
- [ ] **F03 Bounded directory recovery:** on a disposable directory with
  nested files and links, check explicit two-step verified stage publication.
  The source remains. Oversized or unverifiable trees fail closed.
- [ ] **F04 Filesystem failure:** test a removed drive, read-only remount or
  full test filesystem. Inconclusive recovery must keep its evidence rather
  than claim a successful Move.
- [ ] **F05 Power-loss behavior:** **optional, high risk** and only on a
  sacrificial lab system with full backup. Process-crash CI is *not* proof of
  power-loss durability; record this separately or mark NOT TESTED.

## G. Seven rice profiles, RiceBowl and accessibility

- [ ] **G01 Seven rices:** inspect XP Luna, Windows 98, Windows 7, Windows
  10, macOS/Finder, ChromeOS and Frutiger Aero. For each, test default,
  compact/narrow, split and icon/list views with mouse and keyboard.
- [ ] **G02 Fidelity/identity:** compare artwork, toolbar hierarchy, icons,
  rows, hover/focus, menus and density with permitted period references.
  Report specific discrepancies with screenshots and theme name.
- [ ] **G03 RiceBowl decoration boundary:** when running under RiceBowl/labwc,
  window titlebar/buttons remain owned by the window manager, not duplicated
  by RiceField. Switching rices preserves current tabs and pending transfers.
- [ ] **G04 Display scaling:** test 100%, 125%, 150%, HiDPI, 1024×768 and
  dual monitors where available. No clipped critical controls or unreadable
  file rows.
- [ ] **G05 Keyboard/a11y:** tab/focus order, shortcuts, Escape handling,
  screen-reader output where available, high-contrast visibility, reduced
  motion and pointer-only versus keyboard-only paths.

## H. Responsiveness, performance and long-run stability

- [ ] **H01 Startup/idle:** record cold and warm start time, idle RSS, idle
  CPU, machine and display session. Compare against Thunar only on the same
  machine and fixture, not unrelated published benchmarks.
- [ ] **H02 100k files:** enter/search/sort a disposable directory with
  approximately 100,000 entries. Observe UI responsiveness and memory.
- [ ] **H03 Large transfers:** copy a large disposable file and a directory
  tree; record throughput, peak RSS, I/O and progress accuracy on SSD, HDD,
  USB and NAS where possible.
- [ ] **H04 Low memory:** on an actual 4 GB system, repeat startup, tab
  switching, previews and medium transfers without UI freeze/OOM.
- [ ] **H05 Long session:** leave RiceField running at least two hours with
  several network/local tabs and a theme switch. Observe resource growth,
  reconnect behavior, file watcher updates and error states.

## Results to send back

For every failure, provide the **ID** (`B04`, `D07`, etc.), theme, protocol,
expected versus actual result, steps to reproduce and a screenshot or trimmed
log without private paths/credentials. Note `PASS`, `FAIL`, `NOT TESTED` or
`NOT APPLICABLE`; untested is never equivalent to passed.

```text
RiceField commit/version:
Linux distribution / X11 or Wayland:
CPU / RAM / GPU / screen scaling:
USB filesystem / NAS protocol and server:
Passed IDs:
Failed IDs + reproduction:
Not tested / not applicable IDs:
Startup seconds / idle RSS / idle CPU / 100k result:
```

**Stable v1.0.0 exit rule:** all applicable critical `A`, `B`, `D`, `E` and
`F` no-data-loss cases pass, severe UX/performance regressions are resolved,
CI remains green and the remaining protocol/feature limitations are honestly
documented. Never release stable solely because this checklist exists.
