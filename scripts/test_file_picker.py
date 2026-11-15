#!/usr/bin/env python3
"""FileChooser protocol and reversible install regression tests (no desktop needed)."""
from __future__ import annotations

import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest

HERE = Path(__file__).resolve().parent
SCRIPT = HERE / "default_file_picker.py"
spec = importlib.util.spec_from_file_location("filechooser_backend", HERE / "filechooser_backend.py")
backend = importlib.util.module_from_spec(spec)
assert spec and spec.loader
spec.loader.exec_module(backend)


class ProtocolTests(unittest.TestCase):
    def test_open_filters_and_choices_roundtrip(self):
        options = {
            "multiple": True,
            "directory": False,
            "current_folder": b"/tmp/some dir\x00",
            "filters": [("Documents", [(0, "*.docx"), (1, "application/pdf")])],
            "current_filter": ("Documents", [(0, "*.docx"), (1, "application/pdf")]),
            "choices": [("encoding", "Encoding", [("utf8", "UTF-8")], "utf8"),
                        ("readonly", "Read only", [], "false")],
        }
        data = tomllib.loads(backend.serialize_request("open", "Open files", options).decode())
        self.assertEqual(data["folder"], "file:///tmp/some%20dir")
        self.assertEqual(data["filters"][0]["rules"], [[0, "*.docx"], [1, "application/pdf"]])
        self.assertEqual(data["selected_filter"], 0)
        self.assertEqual(data["choices"][1]["options"], [])
        result = backend.parse_result(
            b'uris = ["file:///tmp/a%20b.docx", "file:///tmp/c.docx"]\nselected_filter = 0\n'
            b'[[selections]]\nid = "encoding"\nvalue = "utf8"\n'
            b'[[selections]]\nid = "readonly"\nvalue = "false"\n',
            "open", options,
        )
        self.assertEqual(len(result["uris"]), 2)
        self.assertEqual(result["choices"][0], ("encoding", "utf8"))
        self.assertEqual(result["current_filter"], options["filters"][0])

    def test_save_file_honors_current_file_without_duplicate_name(self):
        data = tomllib.loads(backend.serialize_request("save", "Save", {
            "current_file": b"/tmp/notes a.txt\x00",
        }).decode())
        self.assertEqual(data["current_name"], "notes a.txt")
        self.assertEqual(data["folder"], "file:///tmp")
        self.assertEqual(
            tomllib.loads(backend.serialize_request("save", "Save", {
                "current_file": b"/tmp/existing.txt\x00", "current_name": "override.md",
            }).decode())["current_name"], "override.md"
        )

    def test_save_files_and_validation(self):
        options = {"files": [b"a.txt\x00", b"b.docx\x00"]}
        data = tomllib.loads(backend.serialize_request("save_files", "Save several", options).decode())
        self.assertEqual(data["files"], ["a.txt", "b.docx"])
        output = b'uris = ["file:///tmp/a.txt", "file:///tmp/b.docx"]\n'
        self.assertEqual(len(backend.parse_result(output, "save_files", options)["uris"]), 2)
        for bad in [b"../escape", b"x/y", b"", b".", b".."]:
            with self.assertRaises(ValueError):
                backend.serialize_request("save_files", "Save", {"files": [bad]})
        with self.assertRaises(ValueError):
            backend.parse_result(b'uris = ["https://host/file"]', "open", {})
        with self.assertRaises(ValueError):
            backend.parse_result(b'uris = ["file:///tmp/one", "file:///tmp/two"]', "open", {})

    def test_rejects_oversized_and_untrusted_result(self):
        with self.assertRaises(ValueError):
            backend.quoted("a" * 17000)
        with self.assertRaises(ValueError):
            backend.parse_result(b'uris = ["/tmp/no-uri"]\n', "open", {})
        with self.assertRaises(ValueError):
            backend.parse_result(
                b'uris = ["file:///tmp/a"]\n[[selections]]\nid = "unexpected"\nvalue = "yes"\n',
                "open", {"choices": []})


class SetupTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="ricefield-portal-test-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.home = self.root / "home with spaces"
        self.home.mkdir()
        self.binary = self.root / "bin with spaces"
        shutil.copy2("/bin/true", self.binary)
        self.config = self.home / ".config/xdg-desktop-portal"
        self.config.mkdir(parents=True)
        self.env = {**os.environ, "HOME": str(self.home),
                    "XDG_CONFIG_HOME": str(self.home / ".config"),
                    "XDG_STATE_HOME": str(self.root / "state"),
                    "RICEFIELD_DEFAULTS_TEST_ROOT": "1",
                    "XDG_CURRENT_DESKTOP": "XFCE"}

    def call(self, *args, code=0):
        result = subprocess.run([sys.executable, str(SCRIPT), *args],
                                env=self.env, text=True, capture_output=True, timeout=20)
        self.assertEqual(result.returncode, code, result.stdout + result.stderr)
        return result

    def test_system_install_restores_originals(self):
        root = self.root / "system-root"
        original = "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.ScreenCast=wlr\n"
        existing = root / "etc/xdg-desktop-portal/portals.conf"
        existing.parent.mkdir(parents=True)
        existing.write_text(original)
        command = ("enable", "--scope", "system", "--binary", str(self.binary),
                   "--test-root", str(root))
        self.call(*command)
        self.call(*command)
        self.assertEqual(existing.read_text(), original)
        self.assertTrue((root / "usr/share/xdg-desktop-portal/portals/ricefield.portal").exists())
        self.assertIn("Name=org.freedesktop.impl.portal.desktop.ricefield",
                      (root / "usr/share/dbus-1/services/"
                       "org.freedesktop.impl.portal.desktop.ricefield.service").read_text())
        self.assertIn("filechooser_backend.py",
                      (root / "usr/share/dbus-1/services/"
                       "org.freedesktop.impl.portal.desktop.ricefield.service").read_text())
        self.call("restore", "--scope", "system", "--test-root", str(root))
        self.assertEqual(existing.read_text(), original)
        self.assertFalse((root / "usr/share/xdg-desktop-portal/portals/ricefield.portal").exists())
        self.assertFalse((root / "usr/local/libexec/ricefield/ricefield-picker").exists())

    def test_user_config_and_conflict_detection(self):
        initial = "[preferred]\ndefault=gtk\n"
        file = self.config / "xfce-portals.conf"
        file.write_text(initial)
        self.call("enable")
        self.assertIn("FileChooser=ricefield;*", file.read_text())
        self.assertIn("default=gtk", file.read_text())
        file.write_text(file.read_text() + "org.freedesktop.impl.portal.ScreenCast=gtk\n")
        self.call("restore", code=1)
        self.assertIn("ScreenCast=gtk", file.read_text())

    def test_symlink_refused_and_failed_activation_does_not_touch_target(self):
        file = self.config / "portals.conf"
        target = self.root / "precious"
        target.write_text("[preferred]\ndefault=gtk\n")
        file.symlink_to(target)
        self.call("enable", code=1)
        self.assertEqual(target.read_text(), "[preferred]\ndefault=gtk\n")


if __name__ == "__main__":
    unittest.main(verbosity=2)
