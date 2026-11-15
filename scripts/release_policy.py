#!/usr/bin/env python3
"""Fail-closed RiceField release/version policy, shared by bundling and publish.

Stable 1.1 releases require an owner-reviewed, version-matching acceptance
ledger in the tagged source tree. An RC remains a clearly marked prerelease.
The historical v1.0 main-branch publish path is retained unchanged.
"""
from __future__ import annotations

import argparse
import re
import sys
import tomllib
from pathlib import Path

# Preserve the original 0.3–1.0 package naming contract exactly.
LEGACY_VERSION = re.compile(
    r"(?:0\.[3-9]\.[0-9]+|1\.0\.[0-9]+)(?:-[A-Za-z0-9.-]+)?\Z"
)
# New 1.1 prereleases use intentionally narrow semantic qualifiers.
VERSION_11 = re.compile(
    r"1\.1\.(?:0|[1-9]\d*)(?:-(?:alpha|beta|rc)\.(?:0|[1-9]\d*))?\Z"
)
SIGNOFF_CHECKS = (
    "mint_xfce_filesystem",
    "ricebowl_wayland_session",
    "real_nas_failure_isolation",
    "removable_media_recovery",
    "system_default_and_rollback",
    "keyboard_and_accessibility",
    "release_and_installer",
)


class PolicyError(ValueError):
    pass


def check_version(version: str) -> None:
    if not (LEGACY_VERSION.fullmatch(version) or VERSION_11.fullmatch(version)):
        raise PolicyError(
            f"Unsupported release version {version!r}; accepted 0.3–0.9, 1.0, "
            "and 1.1 with optional alpha/beta/rc numeric qualifiers"
        )


def is_stable_11(version: str) -> bool:
    return version.startswith("1.1.") and "-" not in version


def verify_acceptance(path: Path, version: str) -> None:
    try:
        manifest = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as error:
        raise PolicyError(f"1.1 stable acceptance ledger unavailable: {error}") from error
    release = manifest.get("release")
    checks = manifest.get("checks")
    if not isinstance(release, dict) or not isinstance(checks, dict):
        raise PolicyError("1.1 release acceptance sections missing")
    if (release.get("version") != version
            or release.get("decision") != "approved"
            or release.get("approved_by") != "CopyPasteUser"
            or not isinstance(release.get("approved_at"), str)
            or not release["approved_at"].strip()):
        raise PolicyError("1.1 release has no explicit matching owner sign-off")
    incomplete = [name for name in SIGNOFF_CHECKS if checks.get(name) is not True]
    if incomplete:
        raise PolicyError(
            "1.1 release acceptance is incomplete: " + ", ".join(incomplete)
        )


def publish_decision(version: str, ref_type: str, ref_name: str,
                     acceptance: Path) -> str:
    check_version(version)
    if ref_type == "branch":
        if ref_name != "main":
            raise PolicyError("Publishing from a branch other than main is prohibited")
        if version.startswith("1.1."):
            # A Cargo.toml bump on main may generate a preview artifact, but
            # must not publish a stable 1.1 release without an explicit tag.
            return "SKIP"
        if not re.fullmatch(r"1\.0\.(?:0|[1-9]\d*)", version):
            raise PolicyError("Historical main-branch automatic publish only supports stable 1.0")
        return "PUBLISH"
    if ref_type != "tag" or ref_name != f"v{version}":
        raise PolicyError("The tag must match the exact Cargo.toml version")
    if is_stable_11(version):
        verify_acceptance(acceptance, version)
    return "PUBLISH"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("bundle", "publish"), required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--ref-type", choices=("tag", "branch"))
    parser.add_argument("--ref-name")
    parser.add_argument(
        "--acceptance", type=Path,
        default=Path(__file__).resolve().parent.parent / "docs/RELEASE-1.1-ACCEPTANCE.toml"
    )
    args = parser.parse_args()
    try:
        check_version(args.version)
        if args.mode == "bundle":
            print("BUNDLE")
        else:
            if not args.ref_type or not args.ref_name:
                parser.error("publish requires --ref-type and --ref-name")
            print(publish_decision(args.version, args.ref_type,
                                   args.ref_name, args.acceptance))
        return 0
    except PolicyError as error:
        print(f"RiceField release policy: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
