#!/usr/bin/env python3
"""scripts/patch-blueprint.py - point the Craft recipe for the desktop client at this product's executable.

    python scripts/patch-blueprint.py --blueprint-dir DIR [--oem DIR] [--dry-run]

DIR is the nextcloud-client recipe inside the Craft root, e.g.
<craft root>/etc/blueprints/locations/craft-blueprints-nextcloud/nextcloud-client

Why: the recipe at the pinned commit hard-codes the stock names. Its blacklist.txt keeps only
bin/(nextcloud|nextcloudcmd|QtWebEngineProcess).exe and strips every other .exe from the package, which
would drop the branded executable; createPackage() names the package "nextcloud" / "Nextcloud GmbH".
It also adds the missing "import os" that createPackage() needs at the pinned Craft revision.
This script rewrites exactly those things and nothing else.

Safe to run twice: an already-patched recipe is left alone. If an expected line is neither in its stock
form nor already patched, upstream changed the recipe: the script exits 1 without writing anything, so
the build stops instead of shipping an installer without the application.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

STOCK_BLACKLIST = r"bin/(?!(nextcloud|nextcloudcmd|QtWebEngineProcess)).*\.exe"


def edits(exe: str, vendor: str) -> dict[str, list[tuple[str, str]]]:
    blacklist_new = rf"bin/(?!({exe}|{exe}cmd|QtWebEngineProcess)).*\.exe"
    vendor_py = vendor.replace("\\", "\\\\").replace('"', '\\"')
    return {
        "blacklist.txt": [(STOCK_BLACKLIST, blacklist_new)],
        "nextcloud-client.py": [
            # createPackage() calls os.path.join but the recipe never imports os; it relied on Craft's
            # "from Package.CMakePackageBase import *" re-exporting it, which the pinned Craft no longer does
            # (CI run 38696066283: NameError in createPackage). Upstream's CI never packages, so it never hit it.
            ("import info\n", "import info\nimport os  # added by patch-blueprint.py\n"),
            ('self.defines["appname"] = "nextcloud"', f'self.defines["appname"] = "{exe}"'),
            ('self.defines["company"] = "Nextcloud GmbH"', f'self.defines["company"] = "{vendor_py}"'),
            ('self.applicationExecutable = "nextcloud"', f'self.applicationExecutable = "{exe}"'),
        ],
    }


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="patch the Craft desktop-client recipe for a branded executable")
    ap.add_argument("--blueprint-dir", required=True)
    ap.add_argument("--oem", default=str(Path(__file__).resolve().parents[1]))
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args(argv)
    oem = json.loads((Path(a.oem) / "oem.json").read_text(encoding="utf-8"))
    exe, vendor = oem["executable"], oem["vendor"]
    if not re.fullmatch(r"[a-z][a-z0-9]{1,31}", exe):
        print(f"patch-blueprint: invalid executable name {exe!r}", file=sys.stderr)
        return 1
    bdir = Path(a.blueprint_dir)
    new_text: dict[Path, str] = {}
    for name, pairs in edits(exe, vendor).items():
        path = bdir / name
        if not path.is_file():
            print(f"patch-blueprint: missing {path}", file=sys.stderr)
            return 1
        text = path.read_text(encoding="utf-8")
        for old, new in pairs:
            if new in text:
                print(f"already patched: {name}: {new}")
            elif text.count(old) == 1:
                text = text.replace(old, new)
                print(f"{'would patch' if a.dry_run else 'patch'}: {name}: {old} -> {new}")
            else:
                print(f"patch-blueprint: {name}: expected exactly one '{old}' (found {text.count(old)}); "
                      "the recipe changed upstream, re-verify before building", file=sys.stderr)
                return 1
        new_text[path] = text
    if not a.dry_run:
        for path, text in new_text.items():
            if path.read_text(encoding="utf-8") != text:
                path.write_text(text, encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
