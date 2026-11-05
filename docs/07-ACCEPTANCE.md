# 0.7 preview and metadata — acceptance checklist

Status: **development alpha; owner acceptance pending**. The unresolved
0.6 network/NAS mount and credentials tests are not waived by this milestone.

## Automated checks

- `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test --locked`, release build and the existing Xvfb UI smoke test.
- Images below 12 MiB load as thumbnails in the virtualized Grid view; files
  above the byte limit never enter the decoder. The cache holds at most 96
  entries and only one decoder runs concurrently.
- 32 MiB image and 16 MP pixel limits remain in force for Preview/Quick Look.
  Special files, symlinked large files and binary/control-character text
  cannot sneak past the preview gates.
- PDF, media and archive handling are read-only. The optional `pdftoppm` and
  `ffprobe` tools have bounded process runtime and stdout; archive scans obey
  existing libarchive entry and size limits.
- Properties exposes only xattr names, not secret values. Folder sizes skip
  symlinks and other volumes with 100,000-entry/12-second budgets. SHA-256
  reads the already-open descriptor and uses a 2 GiB/30-second ceiling.

## Physical Linux Mint / X11 acceptance — open

1. Open a grid containing hundreds of JPEG/PNG images, including large and
   corrupt samples. Scroll rapidly: the interface remains responsive;
   thumbnails load lazily and do not endlessly re-decode broken images.
2. Select an image, text/code file, PDF, MP3/FLAC/MP4 and ZIP/TAR; press
   Space. Confirm safe preview or actionable missing-backend messaging; press
   Space/Escape to close, and check that the side preview still works.
3. From Properties, test a folder with symlinks, a regular file, and files
   with POSIX ACL and xattrs. Run folder-size and SHA-256 analysis on demand.
   Validate both results against `du`/`sha256sum` with the documented
   exclusions/budgets; verify no file contents, ACLs or xattrs are changed.
4. Disconnect a network drive during preview or analysis. Confirm UI remains
   responsive and retries do not spawn an unbounded number of workers.
5. Test `poppler-utils`/`ffmpeg` installed and absent. Check memory growth,
   startup and 30-minute idle behavior on the X1 Yoga.
6. Run fuzz/malformed-media fixtures and low-memory soak before the 0.7 gate
   can be called stable. Collect and fix any crashes.

No automatic stable tag is permitted while these checks are pending.
