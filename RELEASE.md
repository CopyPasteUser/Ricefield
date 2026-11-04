# Release process (Linux x86_64)

RiceExplorer 0.5.0-alpha.1 is a **preview candidate**, not signed off for use
with irreplaceable files or as a complete network/archive file manager.
The code can produce a tested distribution bundle, but a green CI run alone
does not satisfy the safety, recovery and Linux Mint hardware gates in
[ROADMAP.md](ROADMAP.md).

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
sha256sum --check rice-explorer-*.tar.gz.sha256
```

The archive contains the binary, an unprivileged installer, an uninstaller,
desktop integration, license and docs. The script refuses a mismatched binary
version or an existing archive; given the same binary and source tree it
produces byte-identical archives. Set `SOURCE_DATE_EPOCH` to the source commit
timestamp when making a reproducible build.

To install a downloaded and SHA-256-verified archive, extract it and run
`bash install.sh` **inside the extracted directory**. The executable goes to
`~/.local/bin` and the launcher and SVG icon to `~/.local/share`. Running
`bash uninstall.sh` there removes only those installed files, retaining
configuration and custom themes. No system file-manager defaults are changed.

The [Linux release bundle workflow](.github/workflows/release.yml) is also
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

Pushing a `v0.3.*`, `v0.4.*` or `v0.5.*` tag triggers a **separate** validation workflow, which
publishes a GitHub Release only after all tests, archive integrity, staging
installation and tag/version match pass. Alpha/pre-release tags are marked
as prereleases. Do **not** create the final tag before these gates pass.

## Before publishing stable v0.4.0

1. Replace the read-only `gio` subprocess bridge with a tested native
   GIO/GVfs implementation, including safe transfer semantics, cancellation,
   error recovery and non-overwrite behavior.
2. Validate UDisks2/Polkit drive capabilities and protected mount/unmount/eject.
   Keyring integration is deferred by owner decision; therefore leave app
   credential persistence and unsafe/interactive remote writes disabled.
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
2. Add in-memory encrypted-archive password handling or explicitly narrow the
   supported-format promise to unencrypted inputs; never store passwords.
3. Exercise UI interaction, cancellation/cleanup, non-overwrite publication and
   low-memory operation. A successful Xvfb window screenshot is only a smoke
   test, not a substitute for those workflows.
4. Carry forward the deferred 0.4 reliability limitations transparently and
   avoid a stable tag until the actually supported scopes are verified.
