#!/usr/bin/env python3
"""Fail-closed RiceField release/version policy, shared by bundling and publish.

1.1 stable releases require an explicit matching tag and the complete
release CI workflow, but no additional manual acceptance ledger. A release
candidate remains a clearly marked prerelease. Historic v1.0 policy remains.
"""
from __future__ import annotations

import argparse
import re
import sys

# Preserve the original 0.3–1.0 package naming contract exactly.
LEGACY_VERSION = re.compile(
    r"(?:0\.[3-9]\.[0-9]+|1\.0\.[0-9]+)(?:-[A-Za-z0-9.-]+)?\Z"
)
# New 1.1 prereleases use intentionally narrow semantic qualifiers.
VERSION_11 = re.compile(
    r"1\.1\.(?:0|[1-9]\d*)(?:-(?:alpha|beta|rc)\.(?:0|[1-9]\d*))?\Z"
)
class PolicyError(ValueError):
    pass


def check_version(version: str) -> None:
    if not (LEGACY_VERSION.fullmatch(version) or VERSION_11.fullmatch(version)):
        raise PolicyError(
            f"Unsupported release version {version!r}; accepted 0.3–0.9, 1.0, "
            "and 1.1 with optional alpha/beta/rc numeric qualifiers"
        )


def publish_decision(version: str, ref_type: str, ref_name: str) -> str:
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
    return "PUBLISH"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("bundle", "publish"), required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--ref-type", choices=("tag", "branch"))
    parser.add_argument("--ref-name")
    args = parser.parse_args()
    try:
        check_version(args.version)
        if args.mode == "bundle":
            print("BUNDLE")
        else:
            if not args.ref_type or not args.ref_name:
                parser.error("publish requires --ref-type and --ref-name")
            print(publish_decision(args.version, args.ref_type,
                                   args.ref_name))
        return 0
    except PolicyError as error:
        print(f"RiceField release policy: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
