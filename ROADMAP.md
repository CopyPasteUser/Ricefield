# RiceExplorer roadmap

## v0.1 — usable foundation

- [x] Separate filesystem worker and theme renderer
- [x] Seven built-in rice identities with three shell layouts
- [x] Async scans, virtual list/icons, tabs, history, search and sort
- [x] Safe new/rename/copy/move-to-same-volume/Trash operations
- [x] Configuration, bookmarks, Linux desktop integration, SVG icon
- [x] Unit tests and Linux CI workflow defined
- [x] Linux CI: format, Clippy, unit tests and optimized build
- [x] Publish under a dedicated GitHub repository
- [ ] Linux Mint X11 visual/keyboard acceptance on real hardware

## v0.2 — desktop-native integration

- [ ] Desktop file clipboard interoperability and drag/drop **out** of RiceExplorer (drop imports already work)
- [ ] Cross-volume move with verified copy, progress, cancellation and rollback
- [ ] Symlink-safe copy and contextual link operations
- [ ] True system MIME icons and thumbnail cache with limits
- [ ] Native mounted-device, removable-media and network-share navigation via GIO
- [ ] Proper focus-trapped dialogs, screen reader pass and i18n foundation

## v0.3 — RiceBowl deep integration

- [ ] RiceBowl IPC for live theme switch without restarting tabs
- [ ] Window-decoration and layout adapters closely matching every rice
- [ ] User-editable theme designer with live preview and export
- [ ] Optional split view and independently scrollable tree navigation
- [ ] Configurable command bars and right-click menu actions

## v1.0 — production file manager

- [ ] Thumbnailing and search benchmarks on 4 GB RAM and HDD
- [ ] Filesystem watcher + bounded caching, graceful offline mounts
- [ ] Undo/recovery journal for safe, supported mutations
- [ ] File properties, permissions, ACLs, archive integrations
- [ ] Signed packages, deterministic releases, Linux Mint/Xfce regression matrix

**Release rule:** Neither theme appearance nor performance targets are claimed verified until the binary is compiled and exercised on real hardware. Thunar remains the fallback until feature and recovery parity is measured.
