#!/usr/bin/env python3
"""scripts/make-setup-icon.py - build bootstrapper/app.ico (the setup program's icon and window logo)
from the theme's colored logo PNG. Deterministic for a given input and Pillow version.

    python scripts/make-setup-icon.py [--png theme/colored/wizard_logo@2x.png] [--out bootstrapper/app.ico]

A missing PNG is not an error: the setup program then has no icon (it still works).
"""
import argparse
import sys
from pathlib import Path

SIZES = [(16, 16), (20, 20), (24, 24), (32, 32), (40, 40), (48, 48), (64, 64), (80, 80), (128, 128), (256, 256)]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--png", default="theme/colored/wizard_logo@2x.png")
    ap.add_argument("--out", default="bootstrapper/app.ico")
    a = ap.parse_args()
    src = Path(a.png)
    if not src.is_file():
        print(f"make-setup-icon: {src} not found; building without an icon")
        return 0
    from PIL import Image  # pinned in the workflow

    img = Image.open(src).convert("RGBA")
    # square canvas, logo centred, transparent margin
    side = max(img.size)
    canvas = Image.new("RGBA", (side, side), (0, 0, 0, 0))
    canvas.paste(img, ((side - img.width) // 2, (side - img.height) // 2))
    canvas = canvas.resize((256, 256), Image.LANCZOS)
    canvas.save(a.out, format="ICO", sizes=SIZES)
    print(f"make-setup-icon: wrote {a.out} from {src}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
