#!/usr/bin/python3
"""Real FileChooser D-Bus round-trip on an isolated dbus-run-session.

The helper executable is a disposable mock picker: no graphical desktop or
personal directories are touched. Requires /usr/bin/python3 with python3-gi.
"""
from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest

import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

ROOT = Path(__file__).resolve().parent
BUS = "org.freedesktop.impl.portal.desktop.ricefield"
PATH = "/org/freedesktop/portal/desktop"
IFACE = "org.freedesktop.impl.portal.FileChooser"
REQUEST = "org.freedesktop.impl.portal.Request"


class DBusIntegration(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="ricefield-dbus-test-")
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        self.fake = self.dir / "fake_picker"
        self.fake.write_text(
            "#!/usr/bin/python3\n"
            "import sys, time\n"
            "data = sys.stdin.buffer.read()\n"
            "if b'title = \"CANCEL\"' in data:\n"
            "    time.sleep(30)\n"
            "elif b'mode = \"save_files\"' in data:\n"
            "    sys.stdout.write('uris = [\"file:///tmp/a.txt\", \"file:///tmp/b.txt\"]\\n')\n"
            "else:\n"
            "    sys.stdout.write('uris = [\"file:///tmp/ricefield%20test.txt\"]\\n')\n"
            "    if b'[[filters]]' in data:\n"
            "        sys.stdout.write('selected_filter = 0\\n')\n"
            "    if b'id = \"readonly\"' in data:\n"
            "        sys.stdout.write('[[selections]]\\nid = \"readonly\"\\nvalue = \"false\"\\n')\n"
        )
        self.fake.chmod(0o700)
        self.backend = subprocess.Popen(
            ["/usr/bin/python3", str(ROOT / "filechooser_backend.py"),
             "--binary", str(self.fake)],
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        )
        self.addCleanup(self.stop_backend)
        self.conn = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        bus = Gio.DBusProxy.new_sync(self.conn, Gio.DBusProxyFlags.NONE,
                                     None, "org.freedesktop.DBus",
                                     "/org/freedesktop/DBus", "org.freedesktop.DBus", None)
        for _ in range(80):
            owned = bus.call_sync("NameHasOwner", GLib.Variant("(s)", (BUS,)),
                                  Gio.DBusCallFlags.NONE, 2000, None).unpack()[0]
            if owned:
                break
            if self.backend.poll() is not None:
                self.fail("FileChooser backend exited before registering D-Bus name")
            time.sleep(0.05)
        else:
            self.fail("FileChooser backend did not register D-Bus name")

    def stop_backend(self):
        if self.backend.poll() is None:
            self.backend.terminate()
        try:
            self.backend.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            self.backend.kill()
            self.backend.communicate(timeout=5)

    def call_picker(self, handle, title, options, method="OpenFile"):
        return self.conn.call_sync(
            BUS, PATH, IFACE, method,
            GLib.Variant("(osssa{sv})", (handle, "com.example.Test", "", title, options)),
            GLib.VariantType.new("(ua{sv})"), Gio.DBusCallFlags.NONE, 10000, None
        ).unpack()

    def test_open_file_returns_uri_and_selected_filter(self):
        filter_value = ("Text files", [(0, "*.txt")])
        code, response = self.call_picker(
            "/org/freedesktop/portal/desktop/request/test/open1", "OPEN",
            {"filters": GLib.Variant("a(sa(us))", [filter_value]),
             "current_filter": GLib.Variant("(sa(us))", filter_value),
             "choices": GLib.Variant("a(ssa(ss)s)", [
                 ("readonly", "Read only", [], "false")])},
        )
        self.assertEqual(code, 0)
        self.assertEqual(response["uris"], ["file:///tmp/ricefield%20test.txt"])
        self.assertEqual(response["choices"], [("readonly", "false")])
        self.assertEqual(response["current_filter"], filter_value)

    def test_save_file_and_multiple_named_files(self):
        code, response = self.call_picker(
            "/org/freedesktop/portal/desktop/request/test/save1", "SAVE",
            {"current_name": GLib.Variant("s", "report.txt")},
            method="SaveFile",
        )
        self.assertEqual(code, 0)
        self.assertEqual(response["uris"], ["file:///tmp/ricefield%20test.txt"])
        code, response = self.call_picker(
            "/org/freedesktop/portal/desktop/request/test/save2", "SAVE FILES",
            {"files": GLib.Variant("aay", [b"a.txt\x00", b"b.txt\x00"])},
            method="SaveFiles",
        )
        self.assertEqual(code, 0)
        self.assertEqual(response["uris"], ["file:///tmp/a.txt", "file:///tmp/b.txt"])

    def test_close_cancels_inflight_request(self):
        handle = "/org/freedesktop/portal/desktop/request/test/cancel1"
        output = []
        failure = []

        def worker():
            try:
                output.append(self.call_picker(handle, "CANCEL", {}))
            except Exception as error:
                failure.append(error)

        thread = threading.Thread(target=worker)
        thread.start()
        try:
            for _ in range(60):
                try:
                    self.conn.call_sync(
                        BUS, handle, REQUEST, "Close", GLib.Variant("()", ()),
                        GLib.VariantType.new("()"), Gio.DBusCallFlags.NONE, 500, None
                    )
                    break
                except GLib.Error:
                    time.sleep(0.05)
            else:
                self.fail("Request.Close was never registered")
            thread.join(timeout=6)
            self.assertFalse(thread.is_alive(), "Cancelled FileChooser never replied")
            self.assertFalse(failure, repr(failure))
            self.assertEqual(output[0][0], 1)
        finally:
            if thread.is_alive():
                self.backend.terminate()
                thread.join(timeout=5)


if __name__ == "__main__":
    if not os.environ.get("DBUS_SESSION_BUS_ADDRESS"):
        raise SystemExit("Requires dbus-run-session; never use a user's desktop bus")
    unittest.main(verbosity=2)
