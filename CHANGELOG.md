# Changelog

## Unreleased — v0.2 stability and v0.3 safety groundwork

- Preserve non-UTF-8 filename bytes when choosing a collision-free copy name.
- Capture the original destination when a New Folder/New File dialog opens.
- Correct the desktop launcher to pass local paths, and validate CLI file URIs.
- Extend Xvfb startup/screenshots to all seven bundled themes.
- Add an uninstall script that preserves user configuration.
- Expand the roadmap with P0/P1/P2 priorities and measurable release gates.
- Introduce versioned shell profiles and configurable top-level UI component
  order for all seven built-in rices, reusing the same file operations.
- Improve search debounce repaint timing and Linux mountinfo path decoding.
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

**Validation:** Earlier feature-branch CI passed formatting, Clippy, unit tests,
release build, CLI and seven-theme Xvfb smoke. This newer metadata/race-safety
increment needs CI verification. Real Linux Mint acceptance is still pending.
These changes are not a v1.0 release.

## 0.2.0

- Local Rust/egui file explorer with seven visual profiles.
- Tabs, folder navigation, selection, file operations, worker scans,
  basic properties and image/text previews.
- Cross-device moves with content verification and initial Linux CI.

The v0.2 baseline still has documented functional and safety gaps; consult
[ROADMAP.md](ROADMAP.md) for the current scope and acceptance criteria.
