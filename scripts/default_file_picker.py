#!/usr/bin/env python3
"""Opt-in, reversible FileChooser backend registration for Mint/Xfce and RiceBowl.

System scope installs the backend under /usr/share (required by the Ubuntu
24.04 xdg-desktop-portal 1.18 broker); no existing portal is removed.
User scope only changes this account's FileChooser preference.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import shlex
import sys

import default_file_manager as transaction

PORTAL_KEY = "org.freedesktop.impl.portal.FileChooser"
PORTAL_BUS = "org.freedesktop.impl.portal.desktop.ricefield"
DESCRIPTOR = ("[portal]\n"
              f"DBusName={PORTAL_BUS}\n"
              f"Interfaces={PORTAL_KEY}\n")


def active_portal_config(config: Path) -> tuple[Path, bytes]:
    """Preserve the active portal routing table, not just its FileChooser key.

    xdg-desktop-portal 1.18 loads a single portals.conf. A new user override
    containing only FileChooser could silently disable ScreenCast/Settings.
    """
    desktops = [name.lower() for name in os.environ.get("XDG_CURRENT_DESKTOP", "").split(":")
                if name and all(ch.isalnum() or ch in "_-" for ch in name)]
    filename = f"{desktops[0]}-portals.conf" if desktops else "portals.conf"
    destination = config / "xdg-desktop-portal" / filename
    existing = transaction.read_optional(destination)
    if existing is not None:
        return destination, existing
    home = Path(os.environ["HOME"])
    data = Path(os.environ.get("XDG_DATA_HOME", str(home / ".local/share")))
    config_dirs = os.environ.get("XDG_CONFIG_DIRS", "/etc/xdg").split(":")
    data_dirs = os.environ.get("XDG_DATA_DIRS", "/usr/local/share:/usr/share").split(":")
    roots = [*(Path(value) / "xdg-desktop-portal" for value in config_dirs if value),
             Path("/etc/xdg-desktop-portal"),
             data / "xdg-desktop-portal",
             *(Path(value) / "xdg-desktop-portal" for value in data_dirs if value)]
    for root in roots:
        for name in [*(f"{desktop}-portals.conf" for desktop in desktops), "portals.conf"]:
            source = root / name
            data_bytes = transaction.read_optional(source)
            if data_bytes is not None:
                return destination, data_bytes
    raise RuntimeError(
        "No existing portal routing config found: refusing to disable other portal backends")


def defaults(user: bool, prefix: Path | None, binary: Path | None) -> dict[Path, tuple[bytes, int]]:
    project = Path(__file__).resolve().parent.parent
    source = project / "scripts/filechooser_backend.py"
    if user:
        home = Path(os.environ["HOME"])
        config = Path(os.environ.get("XDG_CONFIG_HOME", str(home / ".config")))
        if not home.is_absolute() or not config.is_absolute():
            raise RuntimeError("HOME and XDG_CONFIG_HOME must be absolute")
        portal_config, baseline = active_portal_config(config)
        return {portal_config: (
            transaction.update_ini(baseline, PORTAL_KEY, "ricefield;*", "preferred"), 0o600
        )}
    root = prefix or Path("/")
    if binary is None:
        raise RuntimeError("System enable requires --binary")
    if not transaction.regular(binary) or not os.access(binary, os.X_OK):
        raise RuntimeError("Supply a regular executable --binary")
    installed_binary = root / "usr/local/libexec/ricefield/ricefield-picker"
    backend = root / "usr/local/libexec/ricefield/filechooser_backend.py"
    portal = root / "usr/share/xdg-desktop-portal/portals/ricefield.portal"
    service = root / "usr/share/dbus-1/services/org.freedesktop.impl.portal.desktop.ricefield.service"
    exec_line = " ".join(shlex.quote(s) for s in
                         ("/usr/bin/python3",
                          "/usr/local/libexec/ricefield/filechooser_backend.py",
                          "--binary", "/usr/local/libexec/ricefield/ricefield-picker"))
    return {
        installed_binary: (transaction.source_bytes(binary, 256 * 1024 * 1024), 0o755),
        backend: (transaction.source_bytes(source, 1024 * 1024), 0o644),
        portal: (DESCRIPTOR.encode("utf-8"), 0o644),
        service: (("[D-BUS Service]\n"
                   f"Name={PORTAL_BUS}\n"
                   f"Exec={exec_line}\n").encode("utf-8"), 0o644),
    }


def state_dir(user: bool, prefix: Path | None) -> Path:
    if user:
        home = Path(os.environ["HOME"])
        base = Path(os.environ.get("XDG_STATE_HOME", str(home / ".local/state")))
        if not base.is_absolute():
            raise RuntimeError("XDG_STATE_HOME must be absolute")
        return base / "ricefield/default-file-picker"
    return (prefix or Path("/")) / "var/lib/ricefield/default-file-picker"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("enable", "restore", "status"))
    parser.add_argument("--scope", choices=("user", "system"), default="user")
    parser.add_argument("--binary", type=Path,
                        help="Verified RiceField executable for system installation")
    parser.add_argument("--test-root", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    user = args.scope == "user"
    if args.test_root is not None:
        if os.environ.get("RICEFIELD_DEFAULTS_TEST_ROOT") != "1" or not args.test_root.is_absolute():
            parser.error("--test-root requires explicit isolated test mode and absolute path")
    if not user and os.geteuid() != 0 and args.test_root is None:
        parser.error("System scope requires administrator privileges")
    if user and os.geteuid() == 0 and args.test_root is None:
        parser.error("Do not use sudo for user scope")
    if args.binary is not None and user:
        parser.error("--binary requires system scope")
    if not user and args.action == "enable" and args.binary is None:
        parser.error("System installation needs --binary /path/to/ricefield")
    root = state_dir(user, args.test_root)
    if args.action == "status" and not root.exists():
        print("RiceField FileChooser: not managed")
        return 0
    with transaction.locked(root):
        if args.action == "restore":
            transaction.restore(root)
        elif args.action == "enable":
            transaction.activate(root, defaults(user, args.test_root, args.binary))
        else:
            receipt = transaction.load_receipt(root)
            print(f"FileChooser scope: {args.scope}")
            print(f"Receipt: {'present' if receipt else 'absent'}")
            if receipt:
                for item in receipt["items"]:
                    path = Path(item["path"])
                    print(f"{'OK' if transaction.state_of(path)[0] == item['new_sha'] else 'CHANGED'} {path}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, ValueError, UnicodeError) as error:
        print(f"ricefield-file-picker: {error}", file=sys.stderr)
        raise SystemExit(1) from None
