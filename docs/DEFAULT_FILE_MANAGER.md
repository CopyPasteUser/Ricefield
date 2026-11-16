# RiceField as the Linux default file manager

RiceField can become the preferred file manager for XDG directory links,
Xfce's `exo-open --launch FileManager`, RiceBowl shortcuts that delegate to
those mechanisms, and **new** D-Bus `org.freedesktop.FileManager1` activation.
It does not remove or stop Thunar, force-close existing FileManager1 owners,
replace desktop icons, or override another user's explicit application choices.

## User default — your Mint/XFCE session

After installing RiceField normally with `bash install.sh`:

```bash
bash install.sh --default-file-manager-status
bash install.sh --default-file-manager
xdg-mime query default inode/directory
# Expected: ricefield.desktop (if no other higher-priority user override)
# exo-open --launch FileManager should open RiceField on XFCE.
```

The no-argument `bash install.sh` only installs or updates RiceField; switching the default requires the explicit flag above. The status and restore options do not rebuild the application. This is a directory/default-handler integration, not a replacement for native drag-and-drop or non-portal application-specific file chooser dialogs.\n\nThis changes **only the current account** under `$XDG_CONFIG_HOME` and
`$XDG_DATA_HOME`. The script stores original files and their SHA-256 checksums
under `$XDG_STATE_HOME/ricefield/default-file-manager` (mode 0700).
It preserves custom associations for unrelated MIME types and creates an
Xfce helper plus a session D-Bus activation descriptor.

To undo:

```bash
bash install.sh --restore-default-file-manager
```

Restoration refuses to overwrite any managed file edited by another program
since activation. Check the named path and manually reconcile it before
restoring; the receipt and backups are deliberately retained on conflict.
Restore the defaults **before** uninstalling the user-local RiceField launcher.

## Administratively installed default for all accounts

The normal `install.sh` is user-local. An administrator must explicitly install
the **real executable and desktop entry for all users**, not point a system
launcher to one person's home directory:

```bash
# As your normal account, from the RiceField checkout:
bash install.sh
sudo python3 scripts/default_file_manager.py enable --scope system \
  --binary "$HOME/.local/bin/ricefield"

# For the currently logged-in account, user choices take precedence:
python3 scripts/default_file_manager.py enable
xdg-mime query default inode/directory
```

System scope installs RiceField into `/usr/local/bin/ricefield`, its launcher
into `/usr/local/share/applications`, icons and the Xfce helper under
`/usr/local/share`, and its FileManager1 activation descriptor under
`/usr/local/share/dbus-1/services`. It also installs the system-wide XDG
MIME defaults in `/etc/xdg/{mimeapps.list,xfce-mimeapps.list}` and the Xfce
helper in `/etc/xdg/xfce4/helpers.rc`. All replaced files are snapshotted
in `/var/lib/ricefield/default-file-manager` and can be restored with:

```bash
sudo python3 scripts/default_file_manager.py restore --scope system
python3 scripts/default_file_manager.py restore
```

The first command removes only the system files this installer actually
created and restores those it replaced. It never deletes the user's local
RiceField install or Thunar. If a package manager, admin or user changed
one of the managed files after activation, rollback **fails closed**.

### Limits and verification

- **"System-wide" means system default** for accounts without more specific
  user/desktop overrides. The XDG spec gives per-user defaults and earlier
  XDG config directories priority; no safe system-wide setting can override
  those without changing each user's configuration. Set the user default
  explicitly for an already configured login.
- An existing `org.freedesktop.FileManager1` bus owner (often Thunar) keeps
  that name until it exits. The installed D-Bus activation descriptor affects
  **future** activations; it does not steal the running owner's connection.
- RiceBowl's existing `Super+E` command dispatch and desktop shortcuts do not
  need duplicate implementations. The default-handler route is shared with
  Xfce and other Linux applications. RiceBowl theme profiles remain separate.
- To verify, use `xdg-mime query default inode/directory`,
  `gio mime inode/directory`, and `exo-open --launch FileManager` in the real
  desktop session. The final command opens a window; do not run it in a
  headless CI session. `dbus-send`/`busctl` can inspect active FileManager1
  ownership without forcibly replacing the existing process.
- The preferred file manager is **not** the desktop icon manager (xfdesktop)
  and does not rewrite a user's panel launchers that hardcode `thunar`.
- Review and verify the selected compiled executable before running the
  administrative activation. A later user-local update does not automatically
  replace `/usr/local/bin/ricefield`; run an explicitly reviewed upgrade.

The installer uses atomic file replacement, preserved snapshots, locks,
symlink rejection and an all-target rollback preflight. It does not claim a
power-loss-atomic multi-directory transaction; if installation was interrupted,
use `restore` to recover from the durable receipt.
