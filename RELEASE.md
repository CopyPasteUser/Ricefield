# Release process (Linux x86_64)

RiceField 1.0.0-rc.19 is a **development candidate**, not signed off for use
with irreplaceable files or as a complete network/archive file manager.
The code can produce a tested distribution bundle, but a green CI run alone
does not satisfy the safety, recovery and Linux Mint hardware gates in
[ROADMAP.md](ROADMAP.md).

The current candidate scope, actual lab results, remaining code limitations and
post-freeze device checklist are maintained in [docs/1.0-STATUS.md](docs/1.0-STATUS.md).
The actionable owner test cases and reporting template are in
[docs/OWNER-ACCEPTANCE-1.0.md](docs/OWNER-ACCEPTANCE-1.0.md).
The historical version gates below remain evidence, not claims of acceptance.

**Owner's release sequence:** complete the 1.0 code candidate autonomously,
then hand over a concrete Linux Mint/NAS/USB/performance test checklist for
owner acceptance. Do not request manual sign-off during alpha development;
do not tag a stable `v1.0.0` until the post-code-freeze findings are resolved.

## Build a preview bundle

Requires Rust/Cargo, GNU tar, gzip, sha256sum and a Linux x86_64 build host.
Build on Ubuntu 24.04 or an equivalent environment for broad Linux Mint 22.x
compatibility. Distribution dependencies such as graphics libraries still
need to be installed on the target system. Archive operations require the
runtime `libarchive.so.13` library (`libarchive13t64` on Ubuntu 24.04 and
Mint 22); the executable still starts if it is absent but archive actions
report the missing backend.

```bash
cargo test --locked --all-targets
cargo build --locked --release
bash scripts/package-release.sh
cd dist
sha256sum --check ricefield-*.tar.gz.sha256
```

The archive contains the binary, an unprivileged installer, an uninstaller,
desktop integration, license and docs. The script refuses a mismatched binary
version or an existing archive; given the same binary and source tree it
produces byte-identical archives. Set `SOURCE_DATE_EPOCH` to the source commit
timestamp when making a reproducible build.

To install a downloaded and SHA-256-verified archive, extract it and run
`bash install.sh` **inside the extracted directory**. The executable goes to
`~/.local/bin` and the launcher and SVG icon to
`${XDG_DATA_HOME:-$HOME/.local/share}`. The launcher has an absolute executable
path so it works from desktop menus without a modified session `PATH`. Running
`bash uninstall.sh` there removes only those installed files, retaining
configuration and custom themes. No system file-manager defaults are changed.
The installer publishes a new binary by rename, validates existing install
paths against symlink/non-file targets and restores the previous executable,
launcher and icon if registration fails. An interrupted power-loss update
across those separate paths is not guaranteed transactional.

The [Linux release bundle workflow](https://github.com/CopyPasteUser/Ricefield/blob/main/.github/workflows/release.yml) is also
manually dispatchable: it builds, runs unit and GUI smoke tests, creates two
bundles, compares them byte-for-byte, verifies checksums, smoke-tests an
isolated installation and uploads the bundle as an Actions artifact. It does
**not** publish anything on a manual dispatch.

## Before publishing v0.3.0

1. Complete the P0 0.3 data-integrity, recovery and adversarial tests in the
   roadmap. In particular, do not mark recursive source deletion or
   crash recovery safe based only on metadata snapshots or matching bytes.
2. Perform the documented physical Linux Mint X11/Xfce acceptance with real
   permission, ACL, extended-attribute and USB/mount fixtures. Keep Thunar as
   fallback and use disposable files until that work passes.
3. Re-run formatting, Clippy, all tests, CLI and seven-rice Xvfb smoke, then
   inspect CI artifacts. Perform a final dependency/license/secret scan.
4. Review `CHANGELOG.md`, `ROADMAP.md` and README for accurate scope. Only
   then move Cargo/package version to `0.3.0`, commit on `main`, protect and
   push the matching `v0.3.0` tag.

Pushing an enabled `v0.3.*` through `v0.9.*` or `v1.0.*` tag triggers a **separate** validation workflow, which
publishes a GitHub Release only after all tests, archive integrity, staging
installation and tag/version match pass. Alpha/pre-release tags are marked
as prereleases. Do **not** create the final tag before these gates pass.

## Before publishing stable v0.4.0

1. Replace the read-only `gio` subprocess bridge with a tested native
   GIO/GVfs implementation, including safe transfer semantics, cancellation,
   error recovery and non-overwrite behavior.
2. Validate UDisks2/Polkit drive capabilities and protected mount/unmount/eject.
   Keyring persistence is now opt-in by the updated owner decision for alpha.8,
   restricted to the desktop Secret Service and not a plaintext fallback;
   keep unsafe/interactive remote writes disabled pending native GIO tests.
3. Real SMB/SFTP/WebDAV/NFS/MTP, offline/authentication failure and removable
   device tests are deferred by owner decision. They must eventually pass
   before declaring their corresponding capabilities safe for daily use.
4. Only after those gates pass, update the package version to `0.4.0`, rerun
   the entire release workflow and publish the matching `v0.4.0` tag. An
   alpha tag such as `v0.4.0-alpha.1` is explicitly a prerelease and does not
   satisfy this sign-off.

## Before publishing stable v0.5.0

1. Run all five create → inspect → verify → selected extract round-trips,
   plus hostile ZIP/TAR/7z fixtures, decompression limit and interrupted
   extraction cases on Linux Mint with disposable directories.
2. **Implemented in alpha.2:** archive passwords stay in memory for the open
   dialog/worker, and encrypted ZIP creation uses AES-256. Before a stable
   release, verify compatibility with external ZIP/7z/RAR samples on Mint.
   Independent backend tests on libarchive 3.7.4 pass traditional ZIP and
   WinZip AES-128/256, but encrypted 7z/RAR and some ZIP encryption variants
   fail closed; see [interoperability results](docs/ARCHIVE-INTEROPERABILITY.md).
   Keyring integration remains deferred.
3. Exercise UI interaction, cancellation/cleanup, non-overwrite publication and
   low-memory operation. A successful Xvfb window screenshot is only a smoke
   test, not a substitute for those workflows.
4. Carry forward the deferred 0.4 reliability limitations transparently and
   avoid a stable tag until the actually supported scopes are verified.

## Before publishing stable v0.6.0

1. Validate the alpha.2 split view, common tab/navigation/file quick-action
   dispatchers, per-folder drag-in and internal copy-drag, folder tree,
   natural sorting, type filters and cross-restart tab/history recovery.
   External native file drag-out (X11/Wayland) and extraction of the remaining
   shell-specific UI primitives are still open before stable 0.6.
   Alpha.3 adds resilient mounted-folder listing and `/mnt` volume discovery,
   but actual disconnected-device and FUSE testing on Mint remains open.
   Alpha.4 separates file-list painting from actions and extends eight-profile
   keyboard/compact-window smoke checks; actual X11/Wayland pointer and screen
   reader acceptance still requires real-device verification.
   Alpha.5 extracts additional presentation widgets, makes small navigation and
   common dialogs more responsive, and asserts visible split/dialog transitions
   in Xvfb. A native outbound drag source and the [Mint acceptance matrix](docs/0.6-MINT-ACCEPTANCE.md)
   still block stable 0.6.
2. Verify duplication, reordering and reopen with mouse and keyboard in all
   seven skins, including offline remote tabs without blocking launch.
3. Repeat the 0.3/0.4/0.5 safety and physical-device checks before sign-off.
   Keyring and physical Mint/device/disconnection tests are outstanding, not
   silently treated as passing. Alpha.8 adds passive NAS discovery and optional
   Secret Service saving; verify both with a real Synology and an unlocked and
   locked keyring before stable sign-off.
