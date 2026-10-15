#!/usr/bin/env bash
# scripts/apply-oem.sh - apply this repository's OEM theme to an upstream desktop client checkout.
#
# Usage: scripts/apply-oem.sh --src DIR [--oem DIR] [--dry-run] [-h]
#   --src DIR   upstream nextcloud/desktop checkout at the tag in oem.json (required)
#   --oem DIR   this repository's root (default: the parent of scripts/)
#   --dry-run   print what would change; change nothing
#
# What it does (upstream at the pinned tag has no OEM_THEME_DIR option; CMakeLists.txt always
# includes NEXTCLOUD.cmake, so the theme is applied as an overlay):
#   1. refuses unless --src is a git checkout of the commit pinned in oem.json and still has every
#      upstream hook this theme relies on (variable names, theme.qrc entries, MSI artwork paths);
#   2. copies OEM.cmake to <src>/OEM.cmake and appends one include line to <src>/NEXTCLOUD.cmake
#      (once; a second run finds the line and leaves the file alone);
#   3. copies theme/** over <src>/theme/** (same file names as upstream);
#   4. copies installer/windows/{banner,dialog}.bmp to <src>/admin/win/msi/gui/;
#   5. applies patches/*.patch in name order (skipped when already applied).
# Idempotent: running it twice gives the same tree. It only writes inside --src.
# shellcheck disable=SC2016  # the ${...} strings below are literal CMake text, not shell expansions
set -euo pipefail

usage() { sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
die() { printf 'apply-oem: ERROR: %s\n' "$*" >&2; exit 1; }
log() { printf 'apply-oem: %s\n' "$*"; }

OEM="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="" DRY=0
while (($#)); do
  case "$1" in
    --src) SRC="${2:?--src needs a directory}"; shift ;;
    --oem) OEM="${2:?--oem needs a directory}"; shift ;;
    --dry-run) DRY=1 ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
  shift
done
[[ -n "$SRC" ]] || { usage >&2; exit 2; }
[[ -d "$SRC" ]] || die "--src is not a directory: $SRC"
[[ -f "$OEM/oem.json" && -f "$OEM/OEM.cmake" ]] || die "$OEM has no oem.json / OEM.cmake (run gen-oem.py, then export)"

PY=""
for c in python3 python; do
  if command -v "$c" >/dev/null 2>&1 && "$c" -c 'import json' >/dev/null 2>&1; then PY="$c"; break; fi
done
[[ -n "$PY" ]] || die "python 3 not found"
oem_get() { "$PY" -c 'import json,sys; d=json.load(open(sys.argv[1],encoding="utf-8"))
for k in sys.argv[2].split("."): d=d[k]
print(d)' "$OEM/oem.json" "$1" | tr -d '\r'; }

EXE="$(oem_get executable)"
COMMIT="$(oem_get upstream.desktop.commit)"
TAG="$(oem_get upstream.desktop.tag)"
INCLUDE_LINE='include("${CMAKE_SOURCE_DIR}/OEM.cmake")  # OEM theme (scripts/apply-oem.sh)'

# 1. the checkout must be the pinned commit
head="$(git -C "$SRC" rev-parse HEAD 2>/dev/null)" || die "$SRC is not a git checkout"
[[ "$head" == "$COMMIT" ]] || die "$SRC is at $head, oem.json pins $TAG = $COMMIT"

# ... and still offer every hook this theme depends on (re-verify the theme when upstream changes).
need() { grep -qF -- "$2" "$SRC/$1" || die "upstream changed: '$2' not found in $1 (re-verify OEM.cmake against $TAG)"; }
need NEXTCLOUD.cmake 'APPLICATION_SERVER_URL_ENFORCE'
need NEXTCLOUD.cmake 'ENFORCE_VIRTUAL_FILES_SYNC_FOLDER'
need NEXTCLOUD.cmake 'ENFORCE_SINGLE_ACCOUNT'
need NEXTCLOUD.cmake 'WIN_MSI_UPGRADE_CODE'
need NEXTCLOUD.cmake 'APPLICATION_WIZARD_USE_CUSTOM_LOGO'
need CMakeLists.txt 'include(${CMAKE_SOURCE_DIR}/NEXTCLOUD.cmake)'
need CMakeLists.txt 'DISABLE_ACCOUNT_MIGRATION'
need config.h.in 'ENFORCE_SINGLE_ACCOUNT'
need config.h.in 'APPLICATION_SERVER_URL_ENFORCE'
need theme.qrc.in 'theme/colored/wizard_logo.svg'
need theme.qrc.in 'theme/colored/@APPLICATION_ICON_NAME@-icon.svg'
need src/gui/CMakeLists.txt '${APPLICATION_ICON_NAME}-w10startmenu.svg'
need src/gui/CMakeLists.txt '${APPLICATION_EXECUTABLE}.VisualElementsManifest.xml'
need admin/win/msi/CMakeLists.txt 'gui/banner.bmp'
need admin/win/msi/CMakeLists.txt 'gui/dialog.bmp'

copy() { # src dest
  if [[ -f "$2" ]] && cmp -s "$1" "$2"; then return 0; fi
  if ((DRY)); then log "would copy ${1#"$OEM"/} -> ${2#"$SRC"/}"; return 0; fi
  mkdir -p "$(dirname "$2")"
  cp -f "$1" "$2"
  log "copied ${1#"$OEM"/} -> ${2#"$SRC"/}"
}

# 2. OEM.cmake + include hook
copy "$OEM/OEM.cmake" "$SRC/OEM.cmake"
if grep -qF 'OEM.cmake")  # OEM theme' "$SRC/NEXTCLOUD.cmake"; then
  log "NEXTCLOUD.cmake already includes OEM.cmake"
elif ((DRY)); then
  log "would append to NEXTCLOUD.cmake: $INCLUDE_LINE"
else
  printf '\n%s\n' "$INCLUDE_LINE" >>"$SRC/NEXTCLOUD.cmake"
  log "NEXTCLOUD.cmake now includes OEM.cmake"
fi

# 3. theme overlay
n=0
while IFS= read -r -d '' f; do
  copy "$f" "$SRC/theme/${f#"$OEM/theme/"}"; n=$((n + 1))
done < <(find "$OEM/theme" -type f -print0)
log "theme overlay: $n files"
[[ -f "$OEM/theme/colored/$EXE-icon.svg" ]] || die "theme/colored/$EXE-icon.svg missing"

# 4. MSI artwork
copy "$OEM/installer/windows/banner.bmp" "$SRC/admin/win/msi/gui/banner.bmp"
copy "$OEM/installer/windows/dialog.bmp" "$SRC/admin/win/msi/gui/dialog.bmp"

# 5. patches (normally none)
shopt -s nullglob
for p in "$OEM"/patches/*.patch; do
  if git -C "$SRC" apply --reverse --check "$p" >/dev/null 2>&1; then
    log "patch already applied: ${p##*/}"
  elif ((DRY)); then
    git -C "$SRC" apply --check "$p" || die "patch does not apply: ${p##*/}"
    log "would apply patch: ${p##*/}"
  else
    git -C "$SRC" apply "$p" || die "patch failed: ${p##*/}"
    log "applied patch: ${p##*/}"
  fi
done
((DRY)) && log "dry-run: nothing changed"
log "done: $SRC is themed as $(oem_get product_name) ($TAG)"
