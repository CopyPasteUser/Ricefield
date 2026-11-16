# RiceField FileChooser Portal — setup, rollback and limitations

The old RiceField default-manager setting controls **directory navigation**, not
application open/save dialogs. This integration has two independent parts:

1. A short-lived `ricefield --file-picker` Rust/egui UI with TOML request/response
   over private pipes. Supports open, multi-select, folder selection, save,
   SaveFiles, globs/MIME filters and caller-provided choice controls.
2. A separate user-session D-Bus backend at
   `org.freedesktop.impl.portal.desktop.ricefield`. It implements
   `org.freedesktop.impl.portal.FileChooser` (`OpenFile`, `SaveFile`,
   `SaveFiles`) and `org.freedesktop.impl.portal.Request.Close`. The
   xdg-desktop-portal frontend still controls sandbox document grants.

The backend never writes a target file and does not replace
`org.freedesktop.FileManager1`. RiceField Explorer sessions are unaffected.

## Preconditions

Mint 22.x / Ubuntu 24.04 with `xdg-desktop-portal`, `python3-gi` and a working
X11 or Wayland session. Install dependencies if needed:

```bash
sudo apt install xdg-desktop-portal xdg-desktop-portal-gtk python3-gi
```

The service process uses `/usr/bin/python3` with system PyGObject.
A Rust/egui GUI must already run normally.

## Install (explicit opt-in, reversible)

From a trusted RiceField checkout or unpacked release:

```bash
bash install.sh
python3 scripts/default_file_picker.py status
sudo python3 scripts/default_file_picker.py enable --scope system \
  --binary "$HOME/.local/bin/ricefield"
python3 scripts/default_file_picker.py enable
```

**System scope** copies a dedicated binary and the backend Python implementation
to `/usr/local/libexec/ricefield/`, the `.portal` descriptor to
`/usr/share/xdg-desktop-portal/portals/`, and the activation descriptor to
`/usr/share/dbus-1/services/`. Ubuntu 24.04's older portal broker needs the
descriptor in the system directory. It does **not** change the selection
routing for all accounts, nor does it replace `/usr/local/bin/ricefield`.
The system binary is pinned to the reviewed version; a later user-local
RiceField update does not silently update it.

**User scope** only updates the active desktop's portal routing preference in
`$XDG_CONFIG_HOME/xdg-desktop-portal/<desktop>-portals.conf`. If that file
does not exist, the installer copies the active lower-priority portal
configuration before changing only `FileChooser`. It deliberately fails
instead of creating a preference without `ScreenCast`, `Settings`,
`Screenshot` or the desktop's normal default backend routing.

The preference is:

```ini
[preferred]
org.freedesktop.impl.portal.FileChooser=ricefield;*
```

**Mint/XFCE without a vendor `portals.conf`:** the user installer now
checks the installed `gtk.portal` descriptor and only if it advertises
`FileChooser` creates a desktop-specific
`$XDG_CONFIG_HOME/xdg-desktop-portal/xfce-portals.conf`:

```ini
[preferred]
default=gtk;
org.freedesktop.impl.portal.FileChooser=ricefield;gtk;
```

The `default=gtk` route keeps GTK-provided interfaces such as Settings
and OpenURI available instead of accidentally disabling them through a
FileChooser-only override. If `xapp.portal` is installed, its declared
Screenshot/Wallpaper interfaces are explicitly preserved and GTK is used
as the screenshot fallback where available. The installer **does not**
fabricate a fallback on unknown desktops or without a verified GTK
FileChooser backend; it fails without editing the user's routing.

This fallback only applies if **no** existing user, system or vendor routing
configuration was found. If a configuration already exists, its other keys
and explicit backend choices remain unchanged. Restore removes a newly
created XFCE file or restores the exact previous bytes.

Other existing configuration keys are preserved. A conflicting or
ambiguous configuration stops installation without replacing any files.

A new desktop login is recommended after activation so the portal daemon
rereads the descriptors, configuration and graphical session environment.
Do not `systemctl --user restart` all portal backends during an ongoing
screen share or unsaved app interaction.

## Verify

```bash
python3 scripts/default_file_picker.py status
sudo python3 scripts/default_file_picker.py status --scope system
busctl --user --activatable list | grep org.freedesktop.impl.portal.desktop.ricefield
journalctl --user -u xdg-desktop-portal --since "10 min ago" --no-pager
```

Open a portal-aware application's Open dialog and verify that a **RiceField**
picker appears. For a direct backend smoke without a graphical app, run a
separate test session with `dbus-run-session`, Xvfb or a disposable account.
Never run destructive portal activation in your real desktop solely to test CI.

The application's chosen file is returned as a local `file://` URI.
Direct `smb://`, `sftp://` and other remote URIs are intentionally
not returned by the FileChooser backend; network paths must be locally mounted
or exposed through GVfs if the calling application requires filesystem paths.

## Restore

```bash
python3 scripts/default_file_picker.py restore
sudo python3 scripts/default_file_picker.py restore --scope system
```

Both scopes keep original-byte backup snapshots and SHA-256 receipts. Restore
refuses to overwrite any target modified since activation. In a conflict,
review the target, preserve its newer edits, and reconcile explicitly.

The installer does not remove Thunar, GTK dialogs, portal providers or
other installed applications.

## Compatibility and open qualification

- Only **portal-aware** applications are affected. Many GTK applications call
  their own file chooser directly. WordCraft's current file-picker call path
  must be inspected on the host or in its source to verify portal use.
- GTK4 may delegate to the portal; GTK3's native and in-process dialogs may
  require application changes. Setting `inode/directory` does not change this.
- The picker uses a separate window; explicit X11/Wayland transient-parent
  association from `parent_window` is not implemented yet.
- Common glob filters and shared MIME globs from `/usr/share/mime/globs2`
  are supported. Complex bracket expressions and missing MIME definitions
  are not fully supported and require further validation.
- The current UI lists at most 5,000 entries per folder. Large folders need
  pagination; inaccessible directories show an error rather than inventing
  selections.
- SaveFiles reports a collision as an error and requires a different directory
  rather than inventing replacement names or overwriting existing data.
- Multiple-open selection uses Ctrl+click; Escape and Cancel send cancellation.

For the owner acceptance gate, test file open/save, multiple files, directory,
SaveFiles, filters, chooser cancellation, two concurrent requests, external
desktop applications, portal routing, and restore on **Mint XFCE** and
**RiceBowl Wayland**. CI cannot substitute for the real-compositor checks.
