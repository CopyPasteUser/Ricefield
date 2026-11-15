#!/usr/bin/env python3
"""Isolated user/system defaults transaction tests; no real home writes."""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("default_file_manager.py")


class DefaultManagerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="ricefield-default-test-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.home = self.root / "a home with spaces"
        self.home.mkdir()
        self.data = self.home / ".local/share"
        (self.home / ".local/bin").mkdir(parents=True)
        shutil.copy2("/bin/true", self.home / ".local/bin/ricefield")
        (self.data / "applications").mkdir(parents=True)
        (self.data / "applications/ricefield.desktop").write_text(
            "[Desktop Entry]\nExec=ricefield %U\n"
        )
        self.config = self.home / ".config"
        self.config.mkdir()
        self.env = {
            **os.environ,
            "HOME": str(self.home),
            "XDG_CONFIG_HOME": str(self.config),
            "XDG_DATA_HOME": str(self.data),
            "XDG_STATE_HOME": str(self.root / "state"),
            "RICEFIELD_DEFAULTS_TEST_ROOT": "1",
        }

    def call(self, *args, code=0):
        proc = subprocess.run(
            [sys.executable, str(SCRIPT), *args],
            env=self.env, capture_output=True, text=True, timeout=25,
        )
        self.assertEqual(proc.returncode, code, proc.stdout + proc.stderr)
        return proc

    def test_user_enables_and_restores_without_affecting_other_keys(self):
        original = (
            "# original\n[Default Applications]\n"
            "inode/directory=thunar.desktop;\n"
            "text/plain=mousepad.desktop;\n"
        )
        (self.config / "mimeapps.list").write_text(original)
        (self.config / "xfce4").mkdir()
        (self.config / "xfce4/helpers.rc").write_text(
            "WebBrowser=firefox\nFileManager=thunar\n"
        )
        self.call("status")
        self.assertFalse((self.root / "state").exists())
        self.call("enable")
        self.call("enable")  # Idempotent.
        mime = (self.config / "mimeapps.list").read_text()
        self.assertEqual(mime.count("inode/directory="), 1)
        self.assertIn("inode/directory=ricefield.desktop;", mime)
        self.assertIn("text/plain=mousepad.desktop;", mime)
        self.assertIn(
            "FileManager=ricefield",
            (self.config / "xfce4/helpers.rc").read_text(),
        )
        self.assertIn(
            '"%s"',
            (self.data / "xfce4/helpers/ricefield.desktop").read_text(),
        )
        self.assertIn(
            "Name=org.freedesktop.FileManager1",
            (self.data / "dbus-1/services/org.freedesktop.FileManager1.service").read_text(),
        )
        self.call("restore")
        self.assertEqual((self.config / "mimeapps.list").read_text(), original)
        self.assertEqual(
            (self.config / "xfce4/helpers.rc").read_text(),
            "WebBrowser=firefox\nFileManager=thunar\n",
        )
        self.assertFalse((self.config / "xfce-mimeapps.list").exists())
        self.assertFalse(
            (self.data / "dbus-1/services/org.freedesktop.FileManager1.service").exists()
        )
        self.assertTrue((self.home / ".local/bin/ricefield").exists())
        self.call("restore")

    def test_post_activation_edits_block_rollback_and_retain_recovery(self):
        self.call("enable")
        mime = self.config / "mimeapps.list"
        mime.write_text(mime.read_text() + "text/html=firefox.desktop;\n")
        self.call("restore", code=1)
        self.assertIn("firefox.desktop", mime.read_text())
        self.assertTrue(
            (self.root / "state/ricefield/default-file-manager/receipt.json").exists()
        )

    def test_symlink_is_never_replaced(self):
        victim = self.root / "victim"
        victim.write_text("nothing changed")
        (self.config / "mimeapps.list").symlink_to(victim)
        self.call("enable", code=1)
        self.assertEqual(victim.read_text(), "nothing changed")
        self.assertFalse(
            (self.root / "state/ricefield/default-file-manager/receipt.json").exists()
        )

    def test_system_installs_and_restores_disposable_prefix(self):
        prefix = self.root / "fake-system"
        defaults = prefix / "etc/xdg"
        defaults.mkdir(parents=True)
        original = "[Default Applications]\ninode/directory=thunar.desktop;\n"
        (defaults / "mimeapps.list").write_text(original)
        command = (
            "enable", "--scope", "system", "--binary",
            str(self.home / ".local/bin/ricefield"), "--test-root", str(prefix),
        )
        self.call(*command)
        self.call(*command)
        self.assertEqual(
            (prefix / "usr/local/bin/ricefield").read_bytes(),
            (self.home / ".local/bin/ricefield").read_bytes(),
        )
        self.assertIn(
            "Exec=/usr/local/bin/ricefield %U",
            (prefix / "usr/local/share/applications/ricefield.desktop").read_text(),
        )
        self.assertIn(
            "FileManager=ricefield",
            (prefix / "etc/xdg/xfce4/helpers.rc").read_text(),
        )
        self.assertIn(
            "inode/directory=ricefield.desktop;",
            (defaults / "mimeapps.list").read_text(),
        )
        self.call("status", "--scope", "system", "--test-root", str(prefix))
        self.call("restore", "--scope", "system", "--test-root", str(prefix))
        self.assertEqual((defaults / "mimeapps.list").read_text(), original)
        self.assertFalse((prefix / "usr/local/bin/ricefield").exists())
        self.assertFalse(
            (prefix / "usr/local/share/applications/ricefield.desktop").exists()
        )

    def test_updated_binary_is_not_deleted_on_system_rollback(self):
        prefix = self.root / "fake-system"
        prefix.mkdir()
        command = (
            "enable", "--scope", "system", "--binary",
            str(self.home / ".local/bin/ricefield"), "--test-root", str(prefix),
        )
        self.call(*command)
        destination = prefix / "usr/local/bin/ricefield"
        destination.write_bytes(b"managed by different package")
        self.call("restore", "--scope", "system", "--test-root", str(prefix), code=1)
        self.assertEqual(destination.read_bytes(), b"managed by different package")

    def test_duplicate_mime_keys_fail_closed(self):
        (self.config / "mimeapps.list").write_text(
            "[Default Applications]\ninode/directory=first.desktop;\n"
            "inode/directory=second.desktop;\n"
        )
        self.call("enable", code=1)
        self.assertIn("second.desktop", (self.config / "mimeapps.list").read_text())


if __name__ == "__main__":
    unittest.main(verbosity=2)
