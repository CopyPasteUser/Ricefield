# Troubleshooting

[README](../README.md) · [User guide](USAGE.md) · [Release status](1.0-STATUS.md)

## The menu launcher is missing or does not start

First try the installed executable:

```bash
~/.local/bin/ricefield --version
~/.local/bin/ricefield
```

The installer registers an absolute executable path, so the desktop menu does
not need `~/.local/bin` in its `PATH`. It respects `XDG_DATA_HOME`; by default the
launcher is `~/.local/share/applications/ricefield.desktop`. If available,
`gtk-launch ricefield` can provide launcher diagnostics. A stale desktop
menu cache may require logging out and back in.

From a source checkout, rebuild/register with `bash install.sh`. From a verified
binary bundle, run its `bash install.sh`. No default file-manager association
is changed, and Thunar remains available.

## Clipboard or drag-out works only inside RiceField

Install `xclip` for X11 or `wl-clipboard` for Wayland to exchange file selections
with other applications. Without them the internal clipboard still works.
An unresponsive desktop clipboard times out without pasting; retry rather than
assuming a transfer started. Switching folders/tabs cancels a pending paste.

**Drag to another app…** needs an installed `ripdrag` or `dragon` helper. Direct
file-tile dragging out of the application is not available in the current framework.

## A network share will not connect or appear in discovery

Check that the Linux desktop has `gio`, suitable `gvfs-backends` and a working
session D-Bus. Discovery can additionally use `avahi-utils`. A server may not
advertise: enter `smb://server/share` manually and use favorites afterward.

Enter credentials in the connection prompt, not a URI. Saved logins require
`libsecret-tools` and an available Linux Secret Service; unlock the desktop
keyring before retrying. Missing discovery advertisements do not prove the
share is offline. Read-only remote tabs do not support transfers or mutations.

## A mount reports “No such device” or becomes unavailable

RiceField cannot repair a broken system mount. Check whether Linux itself
can enumerate it, replacing `/mnt/share` with the affected path:

```bash
findmnt -T /mnt/share -o TARGET,SOURCE,FSTYPE,OPTIONS
timeout 8s ls -la /mnt/share 2>&1 | head -25
stat -f /mnt/share 2>&1
```

These checks do not change the mount. If `ls` fails too, investigate the device
or mount backend first. Do not force-unmount or automatically remount a live
volume while pending I/O is unresolved. For a supported NAS, a GIO SMB URI can
provide a separate read-only route while diagnosing the system mount.

If Linux can read the folder but the current RiceField build cannot, report
the application version, exact error, mount type and the diagnostic output.
Review output before sharing it: mount sources and paths can be private.

## An archive cannot be opened or decrypted

Archive actions need runtime `libarchive.so.13` (`libarchive13t64` on Mint 22 /
Ubuntu 24.04). The application can start without it, but archive actions fail.
A format being readable does not imply every encrypted variant is supported.
Check the [interoperability results](ARCHIVE-INTEROPERABILITY.md), enter the
password in the masked field and reload the list. Use Verify before extraction.
Existing destinations are not overwritten; select a new extraction folder.

## Results or a requested file seem to be missing

Check **Hidden**, Search and **Filters**. Invalid regex and contradictory ranges
can produce no matches; **Clear filters** resets them. Bounded searches label
partial results. A desktop “Show in folder” request still obeys visibility
filters, so the requested file may need those filters cleared.

## A copy or move stopped, or recovery appears after restart

Review per-file results in **Transfers** before retrying. A failed batch may
have completed earlier items. Cross-filesystem Move intentionally leaves the
original even on success; it is not automatic source cleanup.

After interruption, a hidden `.ricefield-staging-*` directory can remain.
Historical move journals are under `$XDG_STATE_HOME/ricefield/operations/`,
defaulting to `~/.local/state/ricefield/operations/`. Keep evidence while
reviewing source, destination and staging paths.

Recovery inspection and bounded byte comparison are read-only. Matching inode
metadata or bytes does not authorize deletion or prove that paths cannot change.
An offered fresh Copy is a separate confirmed operation, not restart resume or
rollback. Do not remove originals merely to dismiss a recovery warning.

## Updating a source checkout fails

Update a clean checkout with `git pull --ff-only`, then run `bash install.sh`.
Preserve local work if Git reports conflicts. If an old checkout has an
untracked `Cargo.lock` that would be overwritten, back it up outside the
repository before retrying; do not discard unrelated files.

## Reporting a problem

Include `ricefield --version`, distribution/session type (X11 or Wayland),
theme, reproduction steps, exact error and whether a system tool reproduces it.
Use disposable fixtures for destructive/transfer tests. Review logs and
screenshots for private paths or credentials before sharing them.
For vulnerabilities, follow [SECURITY.md](../SECURITY.md).
