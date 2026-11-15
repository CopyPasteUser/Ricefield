#!/usr/bin/env python3
"""Release/version policy tests: explicit tags, no hardware checkbox bureaucracy."""
from __future__ import annotations

import importlib.util
import subprocess
import sys
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("release_policy.py")
spec = importlib.util.spec_from_file_location("ricefield_release_policy", SCRIPT)
assert spec and spec.loader
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)


class ReleasePolicyTests(unittest.TestCase):
    def test_legacy_versions_and_11_stable_or_rc_are_supported(self):
        for version in ("0.3.0", "0.3.0-preview1", "0.9.27", "1.0.0",
                        "1.0.1-custom.7", "1.1.0", "1.1.0-rc.1", "1.1.2-beta.2"):
            with self.subTest(version=version):
                policy.check_version(version)
        for version in ("0.2.0", "1.2.0", "2.0.0", "1.1", "1.1.0-rc",
                        "1.1.0-dev", "1.1.0-rc.01", "1.1.00", "../../tag"):
            with self.subTest(version=version):
                with self.assertRaises(policy.PolicyError):
                    policy.check_version(version)

    def test_historic_main_and_explicit_11_tag_policy(self):
        self.assertEqual(policy.publish_decision("1.0.0", "branch", "main"),
                         "PUBLISH")
        for version in ("1.1.0", "1.1.0-rc.1", "1.1.1"):
            self.assertEqual(policy.publish_decision(version, "branch", "main"),
                             "SKIP")
            self.assertEqual(policy.publish_decision(version, "tag", f"v{version}"),
                             "PUBLISH")
        for version, ref_type, ref in (
            ("1.1.0", "branch", "staging"),
            ("1.0.1", "tag", "v1.0.2"),
            ("1.1.0-rc.1", "tag", "v1.1.0"),
            ("0.9.0", "branch", "main"),
        ):
            with self.subTest(version=version, ref=ref):
                with self.assertRaises(policy.PolicyError):
                    policy.publish_decision(version, ref_type, ref)

    def test_stable_11_does_not_depend_on_manual_attestation_file(self):
        # CI and an exact matching tag remain mandatory in release.yml; this
        # policy intentionally does not require a mutable checkbox ledger.
        self.assertEqual(policy.publish_decision("1.1.0", "tag", "v1.1.0"),
                         "PUBLISH")
        with self.assertRaises(policy.PolicyError):
            policy.publish_decision("1.1.0", "tag", "v1.1.1")

    def test_cli_guards_release_vs_bundle(self):
        def run(*args: str):
            return subprocess.run(
                [sys.executable, str(SCRIPT), *args],
                capture_output=True, text=True, timeout=6,
            )
        self.assertEqual(run("--mode", "bundle", "--version", "1.1.0").returncode, 0)
        for version in ("1.1.0-rc.1", "1.1.0"):
            ok = run("--mode", "publish", "--version", version,
                     "--ref-type", "tag", "--ref-name", f"v{version}")
            self.assertEqual(ok.returncode, 0, ok.stderr)
            self.assertEqual(ok.stdout.strip(), "PUBLISH")
        mismatch = run("--mode", "publish", "--version", "1.1.0",
                       "--ref-type", "tag", "--ref-name", "v1.0.0")
        self.assertEqual(mismatch.returncode, 1)
        self.assertIn("tag must match", mismatch.stderr)
        self.assertEqual(
            run("--mode", "publish", "--version", "1.1.0",
                "--ref-type", "branch", "--ref-name", "main").stdout.strip(),
            "SKIP",
        )
        self.assertNotEqual(
            run("--mode", "publish", "--version", "1.1.0").returncode, 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
