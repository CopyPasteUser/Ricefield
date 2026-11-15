#!/usr/bin/env python3
"""RiceField's opt-in xdg-desktop-portal FileChooser backend.

Runs on the user's session bus. All UI is a separate RiceField process;
request completion and cancellation are marshalled to GLib's main loop.
No file access is granted here: xdg-desktop-portal owns document grants.
"""
from __future__ import annotations

import argparse
import json
import logging
import os
from pathlib import Path
import subprocess
import sys
import threading
import tomllib
from urllib.parse import quote_from_bytes, unquote, urlsplit

LOG = logging.getLogger("ricefield.portal")
BUS_NAME = "org.freedesktop.impl.portal.desktop.ricefield"
OBJECT_PATH = "/org/freedesktop/portal/desktop"
INTERFACE = "org.freedesktop.impl.portal.FileChooser"
REQUEST_INTERFACE = "org.freedesktop.impl.portal.Request"
MAX_ACTIVE = 4
MAX_PAYLOAD = 65536


def quoted(text: str) -> str:
    """A quoted TOML string; JSON's UTF-8 escape syntax is compatible here."""
    if len(text) > 16384 or "\x00" in text:
        raise ValueError("Invalid portal option string")
    return json.dumps(text, ensure_ascii=False)


def path_uri(value: object) -> str | None:
    if not isinstance(value, (bytes, bytearray, list, tuple)):
        return None
    raw = bytes(value).split(b"\x00", 1)[0]
    if not raw.startswith(b"/") or len(raw) > 8192:
        return None
    return "file://" + quote_from_bytes(raw, safe="/")


def filename(value: object) -> str:
    if not isinstance(value, (bytes, bytearray, list, tuple)):
        raise ValueError("Invalid SaveFiles filename")
    raw = bytes(value).split(b"\x00", 1)[0]
    if not raw or raw in (b".", b"..") or b"/" in raw or b"\\" in raw or len(raw) > 255:
        raise ValueError("Invalid SaveFiles filename")
    return raw.decode("utf-8", "strict")


def unpack_variant(value: object) -> object:
    """GVariant a{sv} values normally arrive as nested GLib.Variants."""
    return value.unpack() if hasattr(value, "unpack") else value


def serialize_request(mode: str, title: str, options: dict[str, object]) -> bytes:
    if mode not in ("open", "save", "save_files"):
        raise ValueError("Invalid picker mode")
    lines = [f"mode = {quoted(mode)}", f"title = {quoted(title[:512])}"]
    for key in ("accept_label", "current_name"):
        value = unpack_variant(options.get(key))
        if isinstance(value, str):
            lines.append(f"{key} = {quoted(value[:512])}")
    for key in ("multiple", "directory"):
        value = unpack_variant(options.get(key, False))
        lines.append(f"{key} = {'true' if value is True and mode == 'open' else 'false'}")
    folder = path_uri(unpack_variant(options.get("current_folder")))
    current_file = path_uri(unpack_variant(options.get("current_file")))
    if mode == "save" and current_file:
        folder = current_file.rsplit("/", 1)[0] or folder
        if not isinstance(unpack_variant(options.get("current_name")), str):
            lines.append("current_name = " + quoted(unquote(current_file.rsplit("/", 1)[-1])))
    if folder:
        lines.append(f"folder = {quoted(folder)}")
    filters = unpack_variant(options.get("filters", []))
    if not isinstance(filters, (list, tuple)):
        filters = []
    if len(filters) > 64:
        raise ValueError("Too many filters")
    for entry in filters:
        name, rules = entry
        lines.extend(["[[filters]]", f"name = {quoted(str(name)[:128])}"])
        if len(rules) > 128:
            raise ValueError("Too many filter rules")
        valid_rules = [[int(kind), str(pattern)[:256]] for kind, pattern in rules
                       if int(kind) in (0, 1) and pattern]
        lines.append("rules = [" + ", ".join(
            f"[{kind}, {quoted(pattern)}]" for kind, pattern in valid_rules) + "]")
    current_filter = unpack_variant(options.get("current_filter"))
    if current_filter is not None:
        try:
            idx = list(filters).index(current_filter)
            lines.append(f"selected_filter = {idx}")
        except ValueError:
            pass
    choices = unpack_variant(options.get("choices", []))
    if len(choices) > 32:
        raise ValueError("Too many choice controls")
    for identifier, label, values, selected in choices:
        lines.extend(["[[choices]]", f"id = {quoted(str(identifier))}",
                      f"label = {quoted(str(label)[:128])}",
                      "options = [" + ", ".join(
                          f"[{quoted(str(key))}, {quoted(str(caption))}]" for key, caption in values) + "]",
                      f"selected = {quoted(str(selected))}"])
    if mode == "save_files":
        files = unpack_variant(options.get("files", []))
        if not 0 < len(files) <= 128:
            raise ValueError("SaveFiles requires 1–128 filenames")
        lines.append("files = [" + ", ".join(quoted(filename(item)) for item in files) + "]")
    result = ("\n".join(lines) + "\n").encode("utf-8")
    if len(result) > MAX_PAYLOAD:
        raise ValueError("Portal request is too large")
    return result


def parse_result(output: bytes, mode: str, options: dict[str, object]) -> dict[str, object]:
    data = tomllib.loads(output.decode("utf-8", "strict"))
    uris = data.get("uris")
    if not isinstance(uris, list) or not uris or len(uris) > 128:
        raise ValueError("Picker returned no valid selection")
    if (mode == "save" and len(uris) != 1) or (
        mode == "open" and not unpack_variant(options.get("multiple", False)) and len(uris) != 1
    ):
        raise ValueError("Unexpected number of selected paths")
    if mode == "save_files" and len(uris) != len(unpack_variant(options.get("files", []))):
        raise ValueError("Missing SaveFiles paths")
    for uri in uris:
        if not isinstance(uri, str) or len(uri) > 16384:
            raise ValueError("Invalid URI")
        parsed = urlsplit(uri)
        if (parsed.scheme != "file"
                or parsed.netloc not in ("", "localhost") or not parsed.path.startswith("/")
                or parsed.query or parsed.fragment):
            raise ValueError("Picker returned an unsupported URI")
    selections = data.get("selections", [])
    allowed = {str(item[0]): ({str(v[0]) for v in item[2]} or {"true", "false"})
               for item in unpack_variant(options.get("choices", []))}
    chosen = []
    for entry in selections:
        identifier, value = entry["id"], entry["value"]
        if identifier not in allowed or value not in allowed[identifier]:
            raise ValueError("Picker returned an invalid choice")
        chosen.append((identifier, value))
    result: dict[str, object] = {"uris": uris, "choices": chosen}
    index = data.get("selected_filter")
    filters = unpack_variant(options.get("filters", []))
    if isinstance(index, int) and 0 <= index < len(filters):
        result["current_filter"] = filters[index]
    return result


def run_picker(binary: str, payload: bytes, mode: str, options: dict[str, object],
               register: callable, cancelled: threading.Event) -> tuple[int, dict[str, object]]:
    if cancelled.is_set():
        return 1, {}
    process = subprocess.Popen([binary, "--file-picker"], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               close_fds=True)
    register(process)
    if cancelled.is_set():
        process.terminate()
    try:
        output, _ = process.communicate(payload, timeout=60 * 60)
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate()
        return 2, {}
    if cancelled.is_set():
        return 1, {}
    if process.returncode != 0 or len(output) > MAX_PAYLOAD or not output:
        return 2 if process.returncode != 0 else 1, {}
    return 0, parse_result(output, mode, options)


def serve(binary: str) -> None:
    import gi
    gi.require_version("Gio", "2.0")
    from gi.repository import Gio, GLib

    def interface_xml(name: str, methods: list[str]) -> str:
        return "<interface name='" + name + "'>" + "".join(methods) + "</interface>"

    call_in = "".join(f"<arg name='{name}' type='{kind}' direction='in'/>" for name, kind in
                      (("handle", "o"), ("app_id", "s"), ("parent_window", "s"),
                       ("title", "s"), ("options", "a{sv}")))
    reply_out = "<arg name='response' type='u' direction='out'/><arg name='results' type='a{sv}' direction='out'/>"
    portal_xml = interface_xml(INTERFACE, [
        f"<method name='{method}'>{call_in}{reply_out}</method>"
        for method in ("OpenFile", "SaveFile", "SaveFiles")])
    request_xml = interface_xml(REQUEST_INTERFACE, ["<method name='Close'/>"])
    node = Gio.DBusNodeInfo.new_for_xml(f"<node>{portal_xml}{request_xml}</node>")
    service_iface, request_iface = node.interfaces
    loop = GLib.MainLoop()
    active: dict[str, dict] = {}
    connection_ref = [None]

    def complete(handle: str, code: int, values: dict[str, object]) -> bool:
        item = active.pop(handle, None)
        if item is None:
            return False
        connection = connection_ref[0]
        connection.unregister_object(item["registration"])
        if item["cancelled"].is_set():
            code, values = 1, {}
        result = {}
        if code == 0:
            result["uris"] = GLib.Variant("as", values["uris"])
            result["choices"] = GLib.Variant("a(ss)", values["choices"])
            if "current_filter" in values:
                result["current_filter"] = GLib.Variant("(sa(us))", values["current_filter"])
        item["invocation"].return_value(GLib.Variant("(ua{sv})", (code, result)))
        return False

    def on_request(_conn, _sender, path, _iface, method, _params, invocation):
        if method != "Close" or path not in active:
            invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", "Unknown request")
            return
        item = active[path]
        item["cancelled"].set()
        if item.get("process") is not None:
            item["process"].terminate()
        invocation.return_value(GLib.Variant("()", ()))

    def on_portal(connection, _sender, _path, _iface, method, parameters, invocation):
        if method not in ("OpenFile", "SaveFile", "SaveFiles"):
            invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", "Unknown method")
            return
        handle, app_id, parent_window, title, options = parameters.unpack()
        # The front end owns sandbox identity, parenting and document grants.
        del app_id, parent_window
        if handle in active or len(active) >= MAX_ACTIVE:
            invocation.return_value(GLib.Variant("(ua{sv})", (2, {})))
            return
        mode = {"OpenFile": "open", "SaveFile": "save", "SaveFiles": "save_files"}[method]
        try:
            payload = serialize_request(mode, title, options)
            registration = connection.register_object(handle, request_iface, on_request, None, None)
        except (ValueError, TypeError, UnicodeError, OSError) as error:
            LOG.warning("Rejected FileChooser options: %s", error)
            invocation.return_value(GLib.Variant("(ua{sv})", (2, {})))
            return
        cancelled = threading.Event()
        active[handle] = {"invocation": invocation, "registration": registration,
                          "cancelled": cancelled, "process": None}

        def register(process: subprocess.Popen) -> None:
            GLib.idle_add(lambda: store_process(handle, process))

        def worker():
            try:
                code, result = run_picker(binary, payload, mode, options, register, cancelled)
            except (ValueError, TypeError, UnicodeError, OSError, subprocess.SubprocessError) as error:
                LOG.warning("FileChooser request failed: %s", error)
                code, result = 2, {}
            GLib.idle_add(complete, handle, code, result)

        threading.Thread(target=worker, daemon=True).start()

    def store_process(handle, process):
        item = active.get(handle)
        if item is not None:
            item["process"] = process
            if item["cancelled"].is_set() and process.poll() is None:
                process.terminate()
        return False

    def acquired(connection, _name):
        connection_ref[0] = connection
        connection.register_object(OBJECT_PATH, service_iface, on_portal, None, None)

    def lost(_connection, _name):
        LOG.error("Lost FileChooser D-Bus name")
        loop.quit()

    Gio.bus_own_name(Gio.BusType.SESSION, BUS_NAME, Gio.BusNameOwnerFlags.NONE,
                     acquired, None, lost)
    loop.run()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True, type=Path)
    args = parser.parse_args()
    if not args.binary.is_absolute() or not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error("RiceField binary must be a valid absolute executable path")
    logging.basicConfig(level=logging.WARNING)
    serve(str(args.binary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
