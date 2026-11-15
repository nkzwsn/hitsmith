"""Generates the hitsmith logo: an eighth note whose stem is a hammer handle.

Outputs logo.svg, icon-256.png and icon.ico (16-256 px) next to this script.
Pure Python (no Pillow): shapes are rasterized with supersampling.
Run: python make_logo.py
"""
import math
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))

BG_TOP = (0x3a, 0x26, 0x33)
BG_BOTTOM = (0x1a, 0x12, 0x18)
INK = (0xfa, 0xf2, 0xf6)
FACE = (0xff, 0x66, 0xaa)
SPARK = (0xff, 0xcc, 0x22)

# Geometry on a 256x256 canvas.
HEAD = dict(cx=88, cy=184, rx=42, ry=31, rot=-24)  # note head (tilted ellipse)
STEM = (110, 64, 130, 180)                          # handle: x0, y0, x1, y1
GRIPS = []                                          # (unused) bands cut into the handle
HAMMER = (62, 40, 180, 86, 9)                       # hammer head: x0, y0, x1, y1, radius
PEEN = []                                           # (unused) tapered back end
FACE_CAP = (176, 32, 202, 94, 7)                    # striking face, pink
SPARKS = [((214, 42), (230, 30)), ((218, 63), (236, 63)), ((214, 84), (230, 96))]


def in_round_rect(x, y, r):
    x0, y0, x1, y1, rad = r
    if x < x0 or x > x1 or y < y0 or y > y1:
        return False
    cx = min(max(x, x0 + rad), x1 - rad)
    cy = min(max(y, y0 + rad), y1 - rad)
    return (x - cx) ** 2 + (y - cy) ** 2 <= rad * rad


def in_rect(x, y, r):
    x0, y0, x1, y1 = r
    return x0 <= x <= x1 and y0 <= y <= y1


def in_ellipse(x, y, e):
    a = math.radians(e["rot"])
    dx, dy = x - e["cx"], y - e["cy"]
    u = dx * math.cos(a) + dy * math.sin(a)
    v = -dx * math.sin(a) + dy * math.cos(a)
    return (u / e["rx"]) ** 2 + (v / e["ry"]) ** 2 <= 1.0


def in_poly(x, y, pts):
    inside = False
    j = len(pts) - 1
    for i in range(len(pts)):
        xi, yi = pts[i]
        xj, yj = pts[j]
        if (yi > y) != (yj > y) and x < (xj - xi) * (y - yi) / (yj - yi) + xi:
            inside = not inside
        j = i
    return inside


def near_segment(x, y, seg, width):
    (x0, y0), (x1, y1) = seg
    dx, dy = x1 - x0, y1 - y0
    t = max(0.0, min(1.0, ((x - x0) * dx + (y - y0) * dy) / (dx * dx + dy * dy)))
    px, py = x0 + t * dx, y0 + t * dy
    return (x - px) ** 2 + (y - py) ** 2 <= (width / 2) ** 2


def sample(x, y, sparks):
    """Colour (r, g, b, a) of the logo at canvas point (x, y)."""
    if not in_round_rect(x, y, (0, 0, 256, 256, 56)):
        return (0, 0, 0, 0)
    t = y / 256
    col = tuple(round(BG_TOP[i] * (1 - t) + BG_BOTTOM[i] * t) for i in range(3))
    if sparks and any(near_segment(x, y, s, 8) for s in SPARKS):
        col = SPARK
    ink = (in_ellipse(x, y, HEAD) or (in_rect(x, y, STEM) and not any(in_rect(x, y, g) for g in GRIPS))
           or in_round_rect(x, y, HAMMER) or (PEEN and in_poly(x, y, PEEN)))
    if ink:
        col = INK
    if in_round_rect(x, y, FACE_CAP):
        col = FACE
    return col + (255,)


def render(size, ss=4):
    # Small icons drop the sparks so the silhouette stays crisp.
    sparks = size >= 48
    scale = 256 / size
    rows = []
    for py in range(size):
        row = bytearray()
        for px in range(size):
            acc = [0, 0, 0, 0]
            for sy in range(ss):
                for sx in range(ss):
                    r, g, b, a = sample((px + (sx + 0.5) / ss) * scale, (py + (sy + 0.5) / ss) * scale, sparks)
                    acc[0] += r * a
                    acc[1] += g * a
                    acc[2] += b * a
                    acc[3] += a
            n = ss * ss
            a = acc[3] / n
            if a == 0:
                row += b"\0\0\0\0"
            else:
                row += bytes((round(acc[0] / acc[3]), round(acc[1] / acc[3]), round(acc[2] / acc[3]), round(a)))
        rows.append(bytes(row))
    return rows


def png_bytes(rows, size):
    raw = b"".join(b"\0" + r for r in rows)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def svg():
    e = HEAD
    grips = "".join(f'<rect x="{g[0]}" y="{g[1]}" width="{g[2]-g[0]}" height="{g[3]-g[1]}" fill="url(#bg)"/>' for g in GRIPS)
    sparks = "".join(f'<line x1="{a[0]}" y1="{a[1]}" x2="{b[0]}" y2="{b[1]}"/>' for a, b in SPARKS)
    hx0, hy0, hx1, hy1, hr = HAMMER
    fx0, fy0, fx1, fy1, fr = FACE_CAP
    sx0, sy0, sx1, sy1 = STEM
    peen = " ".join(f"{x},{y}" for x, y in PEEN)
    return f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" role="img" aria-label="hitsmith">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#{BG_TOP[0]:02x}{BG_TOP[1]:02x}{BG_TOP[2]:02x}"/>
      <stop offset="1" stop-color="#{BG_BOTTOM[0]:02x}{BG_BOTTOM[1]:02x}{BG_BOTTOM[2]:02x}"/>
    </linearGradient>
    <clipPath id="tile"><rect width="256" height="256" rx="56"/></clipPath>
  </defs>
  <g clip-path="url(#tile)">
    <rect width="256" height="256" fill="url(#bg)"/>
    <g stroke="#{SPARK[0]:02x}{SPARK[1]:02x}{SPARK[2]:02x}" stroke-width="8" stroke-linecap="round">{sparks}</g>
    <g fill="#{INK[0]:02x}{INK[1]:02x}{INK[2]:02x}">
      <ellipse cx="{e["cx"]}" cy="{e["cy"]}" rx="{e["rx"]}" ry="{e["ry"]}" transform="rotate({e["rot"]} {e["cx"]} {e["cy"]})"/>
      <rect x="{sx0}" y="{sy0}" width="{sx1-sx0}" height="{sy1-sy0}"/>
      <rect x="{hx0}" y="{hy0}" width="{hx1-hx0}" height="{hy1-hy0}" rx="{hr}"/>
      {f'<polygon points="{peen}"/>' if PEEN else ''}
    </g>
    {grips}
    <rect x="{fx0}" y="{fy0}" width="{fx1-fx0}" height="{fy1-fy0}" rx="{fr}" fill="#{FACE[0]:02x}{FACE[1]:02x}{FACE[2]:02x}"/>
  </g>
</svg>
'''


def main():
    with open(os.path.join(HERE, "logo.svg"), "w", encoding="utf-8") as f:
        f.write(svg())
    sizes = [16, 24, 32, 48, 64, 128, 256]
    images = {}
    for s in sizes:
        images[s] = png_bytes(render(s, ss=4 if s >= 64 else 8), s)
        print("rendered", s)
    with open(os.path.join(HERE, "icon-256.png"), "wb") as f:
        f.write(images[256])
    # ICO with PNG-compressed entries (supported since Windows Vista).
    header = struct.pack("<HHH", 0, 1, len(sizes))
    offset = 6 + 16 * len(sizes)
    entries = b""
    blobs = b""
    for s in sizes:
        data = images[s]
        entries += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset + len(blobs))
        blobs += data
    with open(os.path.join(HERE, "icon.ico"), "wb") as f:
        f.write(header + entries + blobs)
    print("wrote logo.svg, icon-256.png, icon.ico")


if __name__ == "__main__":
    main()
