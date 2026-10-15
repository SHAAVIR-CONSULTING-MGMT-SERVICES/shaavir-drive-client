#!/usr/bin/env python3
"""scripts/pin-blueprints.py - pin Craft's blueprint repositories to the commits recorded in oem.json.

    python scripts/pin-blueprints.py --ini craftmaster.ini \
        --kde-branch BRANCH --kde-commit SHA --nc-branch BRANCH --nc-commit SHA

Why: upstream's craftmaster.ini names the blueprint repositories by branch
(craft/craft-blueprints-kde.revision = stable-34.0). Craft re-fetches blueprint repositories on its own
(they are "daily update" packages, and `craft craft` fetches craft/craft-blueprints-kde), and with a
branch it checks out whatever that branch points to at the time. Craft's GitSource treats a revision
setting as a tag/commit to check out, so writing the pinned commit there keeps every later fetch on it.

The edit is exact: each revision line must appear once and name the branch oem.json records (or already
the pinned commit, so a second run changes nothing). Anything else means upstream's settings changed:
the script exits 1 without writing, and the build stops.
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


def pin(text: str, repo: str, branch: str, commit: str) -> str:
    rx = re.compile(rf"^(\s*craft/{re.escape(repo)}\.revision\s*=\s*)(\S+)\s*$", re.M)
    found = rx.findall(text)
    if len(found) != 1:
        raise ValueError(f"expected exactly one craft/{repo}.revision line, found {len(found)}")
    current = found[0][1]
    if current not in (branch, commit):
        raise ValueError(f"craft/{repo}.revision is {current!r}, oem.json records branch {branch!r}")
    return rx.sub(lambda m: m.group(1) + commit, text)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--ini", required=True, type=Path)
    ap.add_argument("--kde-branch", required=True)
    ap.add_argument("--kde-commit", required=True)
    ap.add_argument("--nc-branch", required=True)
    ap.add_argument("--nc-commit", required=True)
    a = ap.parse_args()
    for c in (a.kde_commit, a.nc_commit):
        if not re.fullmatch(r"[0-9a-f]{40}", c):
            print(f"pin-blueprints: not a full commit SHA: {c!r}", file=sys.stderr)
            return 1
    raw = a.ini.read_bytes().decode("utf-8")
    try:
        new = pin(raw, "craft-blueprints-kde", a.kde_branch, a.kde_commit)
        new = pin(new, "craft-blueprints-nextcloud", a.nc_branch, a.nc_commit)
    except ValueError as e:
        print(f"pin-blueprints: {e}; {a.ini} left unchanged", file=sys.stderr)
        return 1
    if new != raw:
        a.ini.write_bytes(new.encode("utf-8"))
    print(f"pin-blueprints: craft-blueprints-kde -> {a.kde_commit}, craft-blueprints-nextcloud -> {a.nc_commit}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
