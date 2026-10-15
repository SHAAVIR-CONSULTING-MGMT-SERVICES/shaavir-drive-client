#!/usr/bin/env bash
# scripts/gen-icons.sh - render the app icon at every size the Windows build produces, for review.
#
# Usage: scripts/gen-icons.sh --out DIR [--oem DIR] [--dry-run] [-h]
#   --out DIR   where the preview PNGs go (created; never inside theme/)
#   --oem DIR   this repository's root (default: the parent of scripts/)
#
# Upstream renders the icon PNGs itself at build time (cmake/modules/GenerateIconsUtils.cmake runs
# Inkscape or rsvg-convert on theme/colored/<icon>-icon.svg at 16 20 24 32 40 48 64 128 256 512 1024 px on
# Windows, and the Start menu tile at 70 and 150 px), so this repository ships only the SVGs. This script
# runs the same converter on the same files with the same size list, so a person can check before a
# release that the icon reads at 16 px on a light and a dark taskbar (doc 05 §4.4). When Python with
# Pillow is available it also writes tray-preview-light.png and tray-preview-dark.png (16/20/24/32 px).
# Needs inkscape or rsvg-convert (SVG_CONVERTER_DIR is honoured like upstream). Exit 1 if neither exists.
set -euo pipefail

usage() { sed -n '2,15p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }
die() { printf 'gen-icons: ERROR: %s\n' "$*" >&2; exit 1; }
OEM="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="" DRY=0
while (($#)); do
  case "$1" in
    --out) OUT="${2:?--out needs a directory}"; shift ;;
    --oem) OEM="${2:?--oem needs a directory}"; shift ;;
    --dry-run) DRY=1 ;;
    -h | --help) usage; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
  shift
done
[[ -n "$OUT" ]] || { usage >&2; exit 2; }
case "$(cd "$(dirname "$OUT")" 2>/dev/null && pwd)/" in "$OEM/theme/"*) die "--out must not be inside theme/" ;; esac

PY=""
for c in python3 python; do
  if command -v "$c" >/dev/null 2>&1 && "$c" -c 'import json' >/dev/null 2>&1; then PY="$c"; break; fi
done
[[ -n "$PY" ]] || die "python 3 not found"
EXE="$("$PY" -c 'import json,sys; print(json.load(open(sys.argv[1],encoding="utf-8"))["executable"])' "$OEM/oem.json" | tr -d '\r')"
ICON="$OEM/theme/colored/$EXE-icon.svg"
TILE="$OEM/theme/colored/$EXE-w10startmenu.svg"
[[ -f "$ICON" ]] || die "missing $ICON"

CONV=""
for c in "${SVG_CONVERTER_DIR:+$SVG_CONVERTER_DIR/inkscape}" inkscape "/c/Program Files/Inkscape/bin/inkscape.exe" \
         "${SVG_CONVERTER_DIR:+$SVG_CONVERTER_DIR/rsvg-convert}" rsvg-convert; do
  [[ -n "$c" ]] || continue
  if command -v "$c" >/dev/null 2>&1 || [[ -x "$c" ]]; then CONV="$c"; break; fi
done
[[ -n "$CONV" ]] || die "no SVG converter: install Inkscape or rsvg-convert (upstream's build needs one too)"

render() { # svg size out
  if ((DRY)); then printf 'would render %s at %spx -> %s\n' "${1##*/}" "$2" "$3"; return 0; fi
  # Same invocation as upstream GenerateIconsUtils.cmake: <converter> -w N -h N in.svg -o out.png
  "$CONV" -w "$2" -h "$2" "$1" -o "$3" >/dev/null 2>&1 || die "$CONV failed on ${1##*/} at $2 px"
}
((DRY)) || mkdir -p "$OUT"
for s in 16 20 24 32 40 48 64 128 256 512 1024; do render "$ICON" "$s" "$OUT/$s-$EXE-icon.png"; done
if [[ -f "$TILE" ]]; then for s in 70 150; do render "$TILE" "$s" "$OUT/$s-$EXE-w10startmenu.png"; done; fi

if ((!DRY)) && "$PY" -c 'import PIL' >/dev/null 2>&1; then
  "$PY" - "$OUT" "$EXE" <<'PY'
import sys
from pathlib import Path
from PIL import Image
out, exe = Path(sys.argv[1]), sys.argv[2]
sizes = [16, 20, 24, 32]
for name, bg in (("light", (243, 243, 243, 255)), ("dark", (32, 32, 32, 255))):
    canvas = Image.new("RGBA", (sum(sizes) + 8 * (len(sizes) + 1), 48), bg)
    x = 8
    for s in sizes:
        icon = Image.open(out / f"{s}-{exe}-icon.png").convert("RGBA")
        canvas.alpha_composite(icon, (x, (48 - s) // 2))
        x += s + 8
    canvas.save(out / f"tray-preview-{name}.png")
PY
fi
printf 'gen-icons: %s preview in %s (converter: %s)\n' "$( ((DRY)) && echo "planned" || echo "rendered")" "$OUT" "${CONV##*/}"
