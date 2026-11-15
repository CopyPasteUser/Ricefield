#!/usr/bin/env python3
"""Opt-in, reversible XDG and Xfce file-manager default integration.

Never removes Thunar or overwrites an edited setting on rollback. System mode
installs a system-owned binary and launcher; user mode only changes defaults.
No privileged command is spawned from this script.
"""
from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import re
import shlex
import shutil
import stat
import sys
import tempfile
from contextlib import contextmanager
from pathlib import Path

DESKTOP_ID = "ricefield.desktop"
HELPER_ID = "ricefield"
MANIFEST_VERSION = 1


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def regular(path: Path) -> bool:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError:
        return False
    if not stat.S_ISREG(mode):
        raise RuntimeError(f"Refusing non-regular or symlinked path: {path}")
    return True


def check_parent(path: Path) -> None:
    """Fail closed on symlinked parent paths, including the destination root."""
    for part in (path, *path.parents):
        if part.is_symlink():
            raise RuntimeError(f"Refusing symlinked directory: {part}")
        if part.exists() and not part.is_dir():
            raise RuntimeError(f"Expected directory: {part}")


def read_optional(path: Path) -> bytes | None:
    if not regular(path):
        return None
    return path.read_bytes()


def update_ini(source: bytes | None, key: str, value: str, section: str | None) -> bytes:
    """Replace exactly one key without losing unrelated values or comments."""
    content = (source or b"").decode("utf-8", "strict")
    newline = "\r\n" if "\r\n" in content else "\n"
    lines = content.splitlines(keepends=True)
    if section is None:
        positions = [i for i, line in enumerate(lines)
                     if re.match(rf"^\s*{re.escape(key)}\s*=", line)]
        if len(positions) > 1:
            raise RuntimeError(f"Duplicate {key} entries; resolve ambiguity manually")
        if positions:
            lines[positions[0]] = f"{key}={value}{newline}"
        else:
            if lines and not lines[-1].endswith(("\n", "\r")):
                lines[-1] += newline
            lines.append(f"{key}={value}{newline}")
        return "".join(lines).encode("utf-8")

    headers = [i for i, line in enumerate(lines)
               if line.strip() == f"[{section}]"]
    if len(headers) > 1:
        raise RuntimeError(f"Duplicate [{section}] sections; resolve manually")
    if not headers:
        if lines and not lines[-1].endswith(("\n", "\r")):
            lines[-1] += newline
        lines.extend([f"[{section}]{newline}", f"{key}={value}{newline}"])
        return "".join(lines).encode("utf-8")
    start = headers[0] + 1
    end = next((i for i in range(start, len(lines))
                if re.match(r"^\s*\[.+\]\s*$", lines[i])), len(lines))
    positions = [i for i in range(start, end)
                 if re.match(rf"^\s*{re.escape(key)}\s*=", lines[i])]
    if len(positions) > 1:
        raise RuntimeError(f"Duplicate {key} entries; resolve ambiguity manually")
    if positions:
        lines[positions[0]] = f"{key}={value}{newline}"
    else:
        if end and not lines[end - 1].endswith(("\n", "\r")):
            lines[end - 1] += newline
        lines.insert(end, f"{key}={value}{newline}")
    return "".join(lines).encode("utf-8")


def helper(binary: Path) -> bytes:
    command = shlex.quote(str(binary))
    # XFCE's exo-helper parses these commands using GShell quoting rules.
    return ("[Desktop Entry]\n"
            "Type=X-XFCE-Helper\n"
            "Name=RiceField\n"
            "NoDisplay=true\n"
            "X-XFCE-Category=FileManager\n"
            "Icon=ricefield\n"
            f"X-XFCE-Commands={command}\n"
            f'X-XFCE-CommandsWithParameter={command} "%s"\n').encode()


def dbus_activation(binary: Path) -> bytes:
    # Session D-Bus starts the service as the login user, not as root,
    # even when a system data directory contains the descriptor.
    return ("[D-BUS Service]\n"
            "Name=org.freedesktop.FileManager1\n"
            f"Exec={shlex.quote(str(binary))} --file-manager-service\n").encode()


def system_launcher(project: Path) -> bytes:
    data = (project / "assets/ricefield.desktop").read_text()
    if "Exec=ricefield %U\n" not in data:
        raise RuntimeError("Unexpected RiceField launcher: update system installer")
    return data.replace("Exec=ricefield %U\n",
                        "Exec=/usr/local/bin/ricefield %U\n").encode()


def settings(user: bool, prefix: Path | None, binary: Path, project: Path) -> dict[Path, tuple[bytes, int]]:
    if user:
        home = Path(os.environ["HOME"])
        config = Path(os.environ.get("XDG_CONFIG_HOME", str(home / ".config")))
        data = Path(os.environ.get("XDG_DATA_HOME", str(home / ".local/share")))
        if not config.is_absolute() or not data.is_absolute() or not home.is_absolute():
            raise RuntimeError("XDG paths and HOME must be absolute")
        launcher = data / "applications/ricefield.desktop"
        if not regular(binary) or not os.access(binary, os.X_OK) or not regular(launcher):
            raise RuntimeError("Install RiceField locally with bash install.sh before making it default")
        changes: dict[Path, tuple[bytes, int]] = {}
        helper_path = data / "xfce4/helpers/ricefield.desktop"
    else:
        root = prefix or Path("/")
        config = root / "etc/xdg"
        data = root / "usr/local/share"
        if not regular(binary) or not os.access(binary, os.X_OK):
            raise RuntimeError("Provide a regular executable RiceField binary with --binary")
        changes = {
            root / "usr/local/bin/ricefield": (binary.read_bytes(), 0o755),
            data / "applications/ricefield.desktop": (system_launcher(project), 0o644),
            data / "icons/hicolor/scalable/apps/ricefield.svg":
                ((project / "assets/ricefield.svg").read_bytes(), 0o644),
            data / "icons/hicolor/256x256/apps/ricefield.png":
                ((project / "assets/ricefield.png").read_bytes(), 0o644),
        }
        helper_path = data / "xfce4/helpers/ricefield.desktop"
        binary = root / "usr/local/bin/ricefield"
    changes[data / "dbus-1/services/org.freedesktop.FileManager1.service"] = (
        dbus_activation(binary), 0o644)
    changes[helper_path] = (helper(binary), 0o644)
    for filename in ("mimeapps.list", "xfce-mimeapps.list"):
        path = config / filename
        changes[path] = (update_ini(read_optional(path), "inode/directory",
                                    "ricefield.desktop;", "Default Applications"),
                         0o644 if not user else 0o600)
    rc = config / "xfce4/helpers.rc"
    changes[rc] = (update_ini(read_optional(rc), "FileManager", HELPER_ID, None),
                   0o644 if not user else 0o600)
    return changes


def atomic_write(path: Path, content: bytes, mode: int) -> None:
    check_parent(path.parent)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".ricefield-default-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(tmp, mode)
        os.replace(tmp, path)
    finally:
        if os.path.lexists(tmp):
            os.unlink(tmp)


def state_dir(user: bool, prefix: Path | None) -> Path:
    if user:
        home = Path(os.environ["HOME"])
        path = Path(os.environ.get("XDG_STATE_HOME", str(home / ".local/state")))
        if not path.is_absolute():
            raise RuntimeError("XDG_STATE_HOME must be absolute")
        return path / "ricefield/default-file-manager"
    return (prefix or Path("/")) / "var/lib/ricefield/default-file-manager"


@contextmanager
def locked(root: Path):
    check_parent(root)
    root.mkdir(parents=True, mode=0o700, exist_ok=True)
    os.chmod(root, 0o700)
    lock_path = root / "lock"
    if lock_path.is_symlink() or (lock_path.exists() and not lock_path.is_file()):
        raise RuntimeError("Refusing non-regular default-manager lock")
    fd = os.open(lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "rb+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        yield


def receipt_file(root: Path) -> Path:
    return root / "receipt.json"


def load_receipt(root: Path) -> dict | None:
    path = receipt_file(root)
    raw = read_optional(path)
    if raw is None:
        return None
    data = json.loads(raw)
    if data.get("version") != MANIFEST_VERSION or not isinstance(data.get("items"), list):
        raise RuntimeError("Unknown receipt version; no files changed")
    return data


def state_of(path: Path) -> tuple[str | None, int | None]:
    data = read_optional(path)
    return (None, None) if data is None else (digest(data), stat.S_IMODE(path.stat().st_mode))


def assert_restorable(receipt: dict, root: Path) -> None:
    for i, item in enumerate(receipt["items"]):
        path = Path(item["path"])
        if state_of(path)[0] not in (item["old_sha"], item["new_sha"]):
            raise RuntimeError(f"Refusing rollback: {path} was edited after activation")
        backup = root / "backups" / str(i)
        if item["old_sha"] is not None:
            raw = read_optional(backup)
            if raw is None or digest(raw) != item["old_sha"]:
                raise RuntimeError("Recovery backup is missing or damaged; refusing rollback")


def restore(root: Path) -> None:
    receipt = load_receipt(root)
    if receipt is None:
        print("No RiceField defaults receipt; nothing to restore")
        return
    assert_restorable(receipt, root)
    for i, item in reversed(list(enumerate(receipt["items"]))):
        path = Path(item["path"])
        if state_of(path)[0] == item["old_sha"]:
            continue
        if item["old_sha"] is None:
            path.unlink()
        else:
            content = (root / "backups" / str(i)).read_bytes()
            atomic_write(path, content, item["old_mode"])
    receipt_file(root).unlink()
    shutil.rmtree(root / "backups")
    print("Restored previous file-manager defaults and installed files")


def activate(root: Path, changes: dict[Path, tuple[bytes, int]]) -> None:
    state = load_receipt(root)
    if state is not None:
        if state.get("phase") == "applied" and all(
                state_of(Path(item["path"]))[0] == item["new_sha"]
                for item in state["items"]):
            print("RiceField is already the managed default; no changes")
            return
        raise RuntimeError("Previous activation receipt exists: restore or inspect it first")
    backups = root / "backups"
    check_parent(backups)
    if backups.exists():
        raise RuntimeError("Unexpected old backup directory; refusing to overwrite it")
    for path in changes:
        check_parent(path.parent)
        regular(path)
    backups.mkdir(mode=0o700)
    items: list[dict] = []
    try:
        for i, (path, (target, mode)) in enumerate(changes.items()):
            previous = read_optional(path)
            old_mode = stat.S_IMODE(path.stat().st_mode) if previous is not None else None
            if previous is not None:
                atomic_write(backups / str(i), previous, 0o600)
            items.append({"path": str(path),
                          "old_sha": digest(previous) if previous is not None else None,
                          "old_mode": old_mode, "new_sha": digest(target), "new_mode": mode})
        receipt = {"version": MANIFEST_VERSION, "phase": "prepared", "items": items}
        atomic_write(receipt_file(root), json.dumps(receipt, indent=2).encode(), 0o600)
        try:
            for path, (content, mode) in changes.items():
                atomic_write(path, content, mode)
        except Exception:
            restore(root)
            raise
        receipt["phase"] = "applied"
        atomic_write(receipt_file(root), json.dumps(receipt, indent=2).encode(), 0o600)
    except Exception:
        if not receipt_file(root).exists():
            shutil.rmtree(backups, ignore_errors=True)
        raise
    print("RiceField defaults activated; original settings preserved for safe restore")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("enable", "restore", "status"))
    parser.add_argument("--scope", choices=("user", "system"), default="user")
    parser.add_argument("--binary", type=Path, help="Source binary for administrator system installation")
    parser.add_argument("--test-root", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    user = args.scope == "user"
    if args.test_root is not None and os.environ.get("RICEFIELD_DEFAULTS_TEST_ROOT") != "1":
        parser.error("--test-root is reserved for isolated test fixtures")
    if args.test_root is not None and not args.test_root.is_absolute():
        parser.error("--test-root must be absolute")
    if not user and os.geteuid() != 0 and args.test_root is None:
        parser.error("system scope requires explicit administrator privileges")
    if user and os.geteuid() == 0 and args.test_root is None and os.environ.get("RICEFIELD_DEFAULTS_TEST_ROOT") != "1":
        parser.error("do not run user defaults with sudo; use your desktop account")
    if user and args.binary is not None:
        parser.error("--binary is only valid for system scope")
    if not user and args.action == "enable" and args.binary is None:
        parser.error("system enable requires --binary /path/to/verified/ricefield")
    root = state_dir(user, args.test_root)
    if args.action == "status" and not root.exists():
        print(f"Scope: {args.scope}\nManaged receipt: no")
        return 0
    with locked(root):
        if args.action == "restore":
            restore(root)
        elif args.action == "status":
            state = load_receipt(root)
            print(f"Scope: {args.scope}")
            print(f"Managed receipt: {'yes' if state else 'no'}")
            if state:
                for item in state["items"]:
                    path = Path(item["path"])
                    print(f"{'OK' if state_of(path)[0] == item['new_sha'] else 'CHANGED'} {path}")
        else:
            binary = (Path(os.environ["HOME"]) / ".local/bin/ricefield") if user else args.binary
            assert binary is not None
            changes = settings(user, args.test_root, binary, Path(__file__).resolve().parent.parent)
            activate(root, changes)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, UnicodeError) as exc:
        print(f"ricefield-defaults: {exc}", file=sys.stderr)
        raise SystemExit(1) from None
