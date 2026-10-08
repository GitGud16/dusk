"""Draws Dusk's logo (docs/THEME.md, "Logo"): the four brand colors as horizontal bands in a
rounded square, violet on top, then magenta, pink and a thin gold stripe.

Writes, from the one geometry below:
  dusk-app/assets/dusk.ico  every size Windows asks an icon for, each drawn on whole pixels
                            rather than scaled down, built into dusk.exe by dusk-app/build.rs
  dusk-app/assets/dusk.svg  the mark as shapes, for the window's icon, the toolbar and About
  docs/images/logo.png      the mark at 256 pixels, for the README

Python's standard library only. Run it from anywhere; it writes beside itself in the repo.
"""

import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# The brand colors, top band first (THEME.md: primary, secondary, alert, signal), and the
# bands' heights as parts of the plate's: 9 + 5 + 4 + 2 = 20.
BANDS = [("#5003C0", 9), ("#AB03A9", 5), ("#FF467A", 4), ("#FFD51E", 2)]
PARTS = sum(parts for _, parts in BANDS)
# The plate's corner radius, as a part of its side.
RADIUS = 0.2
# The sizes Windows asks an icon for: 16 to 40 for the title bar, lists and the taskbar at
# the usual display scales, 48 and 64 for Explorer's views, 256 for its largest.
ICON_SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
# Supersamples per pixel along each axis, for the rounded corners' edge.
SAMPLES = 16


def margin(size):
    """Transparent pixels around the plate: none on the small sizes, which need every
    pixel, and a sixteenth of the side from 32 pixels on, as Windows' own icons leave."""
    return size // 16 if size >= 32 else 0


def boundaries(top, side):
    """Where each band ends, from `top` down a plate of `side` pixels, on whole pixels."""
    ends, parts = [], 0
    for _, band in BANDS:
        parts += band
        ends.append(top + round(side * parts / PARTS))
    return ends


def rgb(hex_color):
    return tuple(int(hex_color[i : i + 2], 16) for i in (1, 3, 5))


def coverage(x, y, left, side, radius):
    """How much of pixel (x, y) the rounded plate covers, 0 to 1. The plate's sides are on
    whole pixels, so only pixels in its corners are part covered."""
    right = left + side
    if x < left or x >= right or y < left or y >= right:
        return 0.0
    near_x = x < left + radius or x + 1 > right - radius
    near_y = y < left + radius or y + 1 > right - radius
    if not (near_x and near_y):
        return 1.0
    inside = 0
    for sy in range(SAMPLES):
        py = y + (sy + 0.5) / SAMPLES
        for sx in range(SAMPLES):
            px = x + (sx + 0.5) / SAMPLES
            if not (left <= px <= right and left <= py <= right):
                continue
            # The nearest corner circle's center, when the point is in a corner square.
            cx = min(max(px, left + radius), right - radius)
            cy = min(max(py, left + radius), right - radius)
            if (px - cx) ** 2 + (py - cy) ** 2 <= radius * radius:
                inside += 1
    return inside / (SAMPLES * SAMPLES)


def draw(size):
    """The logo at `size` pixels: rows of (r, g, b, a)."""
    left = margin(size)
    side = size - 2 * left
    radius = RADIUS * side
    ends = boundaries(left, side)
    rows = []
    for y in range(size):
        band = next((i for i, end in enumerate(ends) if y < end), len(BANDS) - 1)
        r, g, b = rgb(BANDS[band][0])
        row = []
        for x in range(size):
            alpha = round(255 * coverage(x, y, left, side, radius))
            row.append((r, g, b, alpha) if alpha else (0, 0, 0, 0))
        rows.append(row)
    return rows


def png(rows):
    """A PNG file of `rows`, 8-bit RGBA."""
    height, width = len(rows), len(rows[0])
    raw = b"".join(b"\x00" + bytes(c for pixel in row for c in pixel) for row in rows)

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def ico(images):
    """An ICO file of `images`, (size, PNG bytes) pairs; Windows reads PNG entries since
    Vista."""
    directory = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, blobs = b"", b""
    for size, data in images:
        # 256 is written as 0, since the field is a byte.
        side = size % 256
        entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(data), offset)
        blobs += data
        offset += len(data)
    return directory + entries + blobs


def svg():
    """The mark as shapes on a 20-unit square: one unit a part of the bands."""
    ends = boundaries(0, PARTS)
    tops = [0] + ends[:-1]
    rects = "".join(
        f'<rect y="{top}" width="{PARTS}" height="{end - top}" fill="{color}"/>'
        for (color, _), top, end in zip(BANDS, tops, ends)
    )
    radius = RADIUS * PARTS
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{PARTS}" height="{PARTS}" '
        f'viewBox="0 0 {PARTS} {PARTS}"><clipPath id="plate"><rect width="{PARTS}" '
        f'height="{PARTS}" rx="{radius:g}"/></clipPath><g clip-path="url(#plate)">{rects}</g>'
        f"</svg>\n"
    )


def main():
    assets = ROOT / "dusk-app" / "assets"
    assets.mkdir(exist_ok=True)
    images = [(size, png(draw(size))) for size in ICON_SIZES]
    (assets / "dusk.ico").write_bytes(ico(images))
    (assets / "dusk.svg").write_text(svg(), encoding="utf-8", newline="\n")
    images_dir = ROOT / "docs" / "images"
    images_dir.mkdir(parents=True, exist_ok=True)
    (images_dir / "logo.png").write_bytes(dict(images)[256])
    for size, data in images:
        print(f"{size:>3} px: {len(data):>5} bytes, bands {boundaries(margin(size), size - 2 * margin(size))}")


if __name__ == "__main__":
    main()
