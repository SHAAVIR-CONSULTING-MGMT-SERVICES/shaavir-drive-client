#!/usr/bin/env bash
# scripts/check-branding.sh - fail when a stock Nextcloud name or logo would ship (doc 05 §4.4, check 13).
#
# Usage: scripts/check-branding.sh [--oem DIR] [--src DIR] [-h]
#   --oem DIR   this repository's root (default: the parent of scripts/)
#   --src DIR   also check an upstream checkout AFTER scripts/apply-oem.sh: the files the build embeds
#               must be this theme's files
#
# Checks on the theme (always):
#   - identity values in OEM.cmake and oem.json contain no "nextcloud" (product, short, executable,
#     icon, vendor, reverse domain, config name);
#   - no file under theme/ or installer/ has "nextcloud" in its name, except the upstream file names this
#     theme deliberately overrides with its own logo: theme/{colored,white,black}/wizard-nextcloud{.svg,.png,@2x.png};
#   - no text file under theme/ or installer/ mentions "nextcloud";
#   - every theme/installer/defaults file matches the SHA-256 recorded in oem.json (proves it came from the
#     generator and the tenant's assets, not from an upstream copy); license texts are exempt.
# Checks on --src (after apply-oem.sh):
#   - NEXTCLOUD.cmake includes OEM.cmake as its last statement; <src>/OEM.cmake equals ours;
#   - the icon, start-menu, wizard and MSI artwork files the build embeds are byte-identical to ours.
# Read-only. Exit 0 = clean, 1 = findings (printed), 2 = usage.
set -euo pipefail

usage() { sed -n '2,22p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
OEM="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC=""
while (($#)); do
  case "$1" in
    --oem) OEM="${2:?--oem needs a directory}"; shift ;;
    --src) SRC="${2:?--src needs a directory}"; shift ;;
    -h | --help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

PY=""
for c in python3 python; do
  if command -v "$c" >/dev/null 2>&1 && "$c" -c 'import json, hashlib' >/dev/null 2>&1; then PY="$c"; break; fi
done
[[ -n "$PY" ]] || { echo "check-branding: python 3 not found" >&2; exit 2; }
[[ -f "$OEM/oem.json" && -f "$OEM/OEM.cmake" ]] || { echo "check-branding: $OEM has no oem.json / OEM.cmake" >&2; exit 2; }

FAIL=0
bad() { FAIL=1; printf 'FAIL  %s\n' "$*"; }
okay() { printf 'ok    %s\n' "$*"; }

# 1. identity values
ident="$(grep -E '^set\( *(APPLICATION_(NAME|SHORTNAME|EXECUTABLE|CONFIG_NAME|ICON_NAME|VENDOR|REV_DOMAIN|REV_DOMAIN_DBUS|VIRTUALFILE_SUFFIX)|LINUX_PACKAGE_SHORTNAME|LINUX_APPLICATION_ID) ' "$OEM/OEM.cmake" || true)"
[[ -n "$ident" ]] || bad "OEM.cmake: no identity variables found"
if grep -iE 'next *cloud' <<<"$ident"; then bad "OEM.cmake identity values mention Nextcloud (lines above)"; else okay "OEM.cmake identity values"; fi
json_hits="$("$PY" - "$OEM/oem.json" <<'PY'
import json, re, sys
d = json.load(open(sys.argv[1], encoding="utf-8"))
for k in ("product_name", "shortname", "executable", "vendor", "rev_domain"):
    if re.search(r"next\s*cloud", str(d.get(k, "")), re.I):
        print(f"{k}={d[k]}")
PY
)"
if [[ -n "$json_hits" ]]; then bad "oem.json: $json_hits"; else okay "oem.json identity values"; fi

# 2. file names and text content under theme/ installer/
allowed_name() { [[ "$1" =~ ^theme/(colored|white|black)/wizard-nextcloud(\.svg|\.png|@2x\.png)$ ]]; }
names=0 texts=0
while IFS= read -r -d '' f; do
  rel="${f#"$OEM/"}"
  base="${rel##*/}"
  if [[ "$base" =~ ^(LICENSE|COPYING)$ || "$base" == *.license ]]; then continue; fi
  if [[ "${rel,,}" == *nextcloud* ]] && ! allowed_name "$rel"; then bad "stock file name: $rel"; names=1; fi
  case "$rel" in
    *.svg | *.xml | *.txt | *.qrc | *.cmake | *.json)
      if grep -qiE 'next *cloud' "$f"; then bad "text mentions Nextcloud: $rel"; texts=1; fi ;;
  esac
done < <(find "$OEM/theme" "$OEM/installer" -type f -print0 2>/dev/null)
((names)) || okay "file names under theme/ and installer/"
((texts)) || okay "text content under theme/ and installer/"

# 3. hashes recorded by the generator
hash_rc=0
hash_out="$("$PY" - "$OEM" <<'PY'
import hashlib, json, sys
from pathlib import Path
root = Path(sys.argv[1])
d = json.loads((root / "oem.json").read_text(encoding="utf-8"))
files = d.get("files", {})
bad = 0
for rel, want in sorted(files.items()):
    if not rel.startswith(("theme/", "installer/", "defaults/")) and rel != "OEM.cmake":
        continue
    p = root / rel
    if not p.is_file():
        print(f"missing: {rel}"); bad = 1; continue
    if hashlib.sha256(p.read_bytes()).hexdigest() != want:
        print(f"changed after generation: {rel}"); bad = 1
for p in sorted(list((root / "theme").rglob("*")) + list((root / "installer").rglob("*"))):
    if p.is_file() and p.relative_to(root).as_posix() not in files:
        print(f"not from the generator: {p.relative_to(root).as_posix()}"); bad = 1
sys.exit(bad)
PY
)" || hash_rc=$?
if ((hash_rc == 0)); then okay "theme, installer and defaults files match oem.json"
else bad "generator hashes:"; printf '      %s\n' "$hash_out"; fi

# 4. the upstream checkout after apply-oem.sh
if [[ -n "$SRC" ]]; then
  EXE="$("$PY" -c 'import json,sys; print(json.load(open(sys.argv[1],encoding="utf-8"))["executable"])' "$OEM/oem.json" | tr -d '\r')"
  last="$(grep -vE '^[[:space:]]*(#.*)?$' "$SRC/NEXTCLOUD.cmake" | tail -n 1)"
  if [[ "$last" == *'/OEM.cmake")'* ]]; then okay "NEXTCLOUD.cmake ends with the OEM.cmake include"; else bad "NEXTCLOUD.cmake does not end with the OEM.cmake include (last: $last)"; fi
  same() { if cmp -s "$1" "$2"; then okay "embedded: ${2#"$SRC"/}"; else bad "not ours: ${2#"$SRC"/}"; fi; }
  same "$OEM/OEM.cmake" "$SRC/OEM.cmake"
  for rel in "colored/$EXE-icon.svg" "colored/$EXE-w10startmenu.svg" colored/wizard_logo.svg colored/wizard_logo.png \
             colored/wizard_logo@2x.png "$EXE.VisualElementsManifest.xml"; do
    same "$OEM/theme/$rel" "$SRC/theme/$rel"
  done
  for flavor in colored white black; do
    for ext in .svg .png @2x.png; do same "$OEM/theme/$flavor/wizard-nextcloud$ext" "$SRC/theme/$flavor/wizard-nextcloud$ext"; done
  done
  same "$OEM/installer/windows/banner.bmp" "$SRC/admin/win/msi/gui/banner.bmp"
  same "$OEM/installer/windows/dialog.bmp" "$SRC/admin/win/msi/gui/dialog.bmp"
fi

if ((FAIL)); then echo "RESULT: FAIL"; exit 1; fi
echo "RESULT: PASS"
