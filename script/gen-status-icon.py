#!/usr/bin/env python3
"""Renders the menu bar glyph to its two PNG sizes.

The glyph is crates/loctel-macos/assets/status-icon.svg (the three-node mark
from the design mockups: three rings joined by two lines, on a 16-unit grid,
1.5 units of stroke). This script draws exactly that geometry, black on
transparent because a macOS template image is coloured by AppKit from its
alpha channel alone, and writes:

  crates/loctel-macos/assets/status-icon.png     18 x 18  (1x)
  crates/loctel-macos/assets/status-icon@2x.png  36 x 36  (2x)

Run it after changing the SVG's geometry (it does not parse the SVG, so change
GLYPH below to match). Needs Pillow: `pip install pillow`.
"""

import math
from pathlib import Path

from PIL import Image, ImageDraw

# The SVG's geometry, in its 16-unit grid.
GRID = 16.0
STROKE = 1.5
RADIUS = 2.0
CIRCLES = [(3.5, 8.0), (12.5, 3.5), (12.5, 12.5)]
LINES = [((5.3, 7.1), (10.7, 4.4)), ((5.3, 8.9), (10.7, 11.6))]

SUPERSAMPLE = 16  # draw big, then shrink for smooth edges
OUT = Path(__file__).resolve().parent.parent / "crates" / "loctel-macos" / "assets"


def render(pixels: int) -> Image.Image:
    scale = pixels / GRID * SUPERSAMPLE
    size = pixels * SUPERSAMPLE
    mask = Image.new("L", (size, size), 0)
    draw = ImageDraw.Draw(mask)

    for cx, cy in CIRCLES:
        outer = (RADIUS + STROKE / 2) * scale
        inner = (RADIUS - STROKE / 2) * scale
        draw.ellipse(
            [cx * scale - outer, cy * scale - outer, cx * scale + outer, cy * scale + outer],
            fill=255,
        )
        draw.ellipse(
            [cx * scale - inner, cy * scale - inner, cx * scale + inner, cy * scale + inner],
            fill=0,
        )

    for (x1, y1), (x2, y2) in LINES:
        dx, dy = x2 - x1, y2 - y1
        length = math.hypot(dx, dy)
        # Half the stroke, perpendicular to the line (butt caps, like SVG).
        px, py = -dy / length * STROKE / 2, dx / length * STROKE / 2
        draw.polygon(
            [
                ((x1 + px) * scale, (y1 + py) * scale),
                ((x2 + px) * scale, (y2 + py) * scale),
                ((x2 - px) * scale, (y2 - py) * scale),
                ((x1 - px) * scale, (y1 - py) * scale),
            ],
            fill=255,
        )

    alpha = mask.resize((pixels, pixels), Image.BOX)  # exact coverage, no ringing
    image = Image.new("RGBA", (pixels, pixels), (0, 0, 0, 0))
    image.putalpha(alpha)
    return image


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    render(18).save(OUT / "status-icon.png", optimize=True)
    render(36).save(OUT / "status-icon@2x.png", optimize=True)


if __name__ == "__main__":
    main()
