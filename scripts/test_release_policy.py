#!/usr/bin/env python3
"""Test release policy without generating tags, publishing or changing Cargo."""
from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("release_policy.py")
spec = importlib.util.spec_from_file_location("ricefield_release_policy", SCRIPT)
assert spec and spec.loader
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)


def manifest(version: str, approve: bool, *, missing: str = "") -> str:
    values = "\n".join(
        f"{key} = {str(approve and key != missing).lower()}"
        for key in policy.SIGNOFF_CHECKS
    )
    return (
        f'[release]\nversion = "{version}"\n'
        f'decision = "{"approved" if approve else "pending"}"\n'
        f'approved_by = "{"CopyPasteUser" if approve else ""}"\n'
        f'approved_at = "{"2026-11-15" if approve else ""}"\n'
        f"\n[checks]\n{values}\n"
    )


class ReleasePolicyTests(unittest.TestCase):
    def test_historical_versions_and_valid_rc_are_supported(self):
        for version in ("0.3.0", "0.3.0-preview1", "0.9.27", "1.0.0",
                        "1.0.1-custom.7", "1.1.0", "1.1.0-rc.1", "1.1.2-beta.2"):
            policy.check_version(version)
        for version in ("0.2.0", "1.2.0", "2.0.0", "1.1", "1.1.0-rc",
                        "1.1.0-dev", "1.1.0-rc.01", "1.1.00", "../../tag"):
            with self.subTest(version=version):
                with self.assertRaises(policy.PolicyError):
                    policy.check_version(version)

    def test_legacy_main_release_and_explicit_11_tag_policy(self):
        path = Path("/does-not-exist")
        self.assertEqual(policy.publish_decision("1.0.0", "branch", "main", path),
                         "PUBLISH")
        self.assertEqual(policy.publish_decision("1.1.0", "branch", "main", path),
                         "SKIP")
        self.assertEqual(policy.publish_decision("1.1.0-rc.1", "branch", "main", path),
                         "SKIP")
        self.assertEqual(policy.publish_decision("1.1.0-rc.1", "tag", "v1.1.0-rc.1", path),
                         "PUBLISH")
        for version, ref_type, ref in (
            ("1.1.0", "branch", "staging"),
            ("1.0.1", "tag", "v1.0.2"),
            ("1.1.0-rc.1", "tag", "v1.1.0"),
            ("0.9.0", "branch", "main"),
        ):
            with self.assertRaises(policy.PolicyError):
                policy.publish_decision(version, ref_type, ref, path)

    def test_stable_11_is_blocked_until_every_owner_gate_is_approved(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "acceptance.toml"
            path.write_text(manifest("1.1.0", False))
            with self.assertRaises(policy.PolicyError):
                policy.publish_decision("1.1.0", "tag", "v1.1.0", path)
            path.write_text(manifest("1.1.0", True, missing="real_nas_failure_isolation"))
            with self.assertRaises(policy.PolicyError):
                policy.publish_decision("1.1.0", "tag", "v1.1.0", path)
            path.write_text(manifest("1.1.1", True))
            with self.assertRaises(policy.PolicyError):
                policy.publish_decision("1.1.0", "tag", "v1.1.0", path)
            path.write_text(manifest("1.1.0", True))
            self.assertEqual(
                policy.publish_decision("1.1.0", "tag", "v1.1.0", path), "PUBLISH")

    def test_cli_bundle_publish_and_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "approval.toml"
            path.write_text(manifest("1.1.0", False))
            def run(*args: str):
                return subprocess.run(
                    [sys.executable, str(SCRIPT), *args],
                    capture_output=True, text=True, timeout=6,
                )
            self.assertEqual(run("--mode", "bundle", "--version", "1.1.0-rc.1").returncode, 0)
            self.assertEqual(
                run("--mode", "publish", "--version", "1.1.0-rc.1",
                    "--ref-type", "tag", "--ref-name", "v1.1.0-rc.1").stdout.strip(),
                "PUBLISH")
            blocked = run("--mode", "publish", "--version", "1.1.0",
                          "--ref-type", "tag", "--ref-name", "v1.1.0",
                          "--acceptance", str(path))
            self.assertEqual(blocked.returncode, 1)
            self.assertIn("no explicit matching owner sign-off", blocked.stderr)
            self.assertEqual(
                run("--mode", "publish", "--version", "1.1.0",
                    "--ref-type", "branch", "--ref-name", "main").stdout.strip(),
                "SKIP")


if __name__ == "__main__":
    unittest.main(verbosity=2)
