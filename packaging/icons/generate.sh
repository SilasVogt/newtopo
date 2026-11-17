#!/usr/bin/env bash
# Regenerate the committed app icons from assets/icon.svg.
# Needs: rsvg-convert (librsvg) and python3 with Pillow.
# Output (committed): packaging/icons/png/newtopo-<size>.png, newtopo.ico, newtopo.icns
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
svg="$root/assets/icon.svg"
out="$here/png"
mkdir -p "$out"
for s in 16 24 32 48 64 128 256 512 1024; do
  rsvg-convert -w "$s" -h "$s" "$svg" -o "$out/newtopo-$s.png"
done
python3 -I - "$here" <<'PY'
import sys
from pathlib import Path
from PIL import Image
here = Path(sys.argv[1])
png = here / "png"
big = Image.open(png / "newtopo-1024.png").convert("RGBA")
# Windows .ico: exact renders for the classic sizes (256 stored as PNG).
ico_sizes = [(s, s) for s in (16, 24, 32, 48, 64, 128, 256)]
ico_imgs = [Image.open(png / f"newtopo-{w}.png").convert("RGBA") for w, _ in ico_sizes]
ico_imgs[-1].save(here / "newtopo.ico", sizes=ico_sizes, append_images=ico_imgs[:-1])
# macOS .icns: exact renders for every standard size (16..1024, @1x/@2x).
others = [Image.open(png / f"newtopo-{w}.png").convert("RGBA") for w in (16, 32, 64, 128, 256, 512)]
big.save(here / "newtopo.icns", append_images=others)
PY
echo "icons written to $here"
