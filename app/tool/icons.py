#!/usr/bin/env python3
"""Anvil's icons, drawn in pure Python so anyone can draw them again:
`python3 tool/icons.py`.

- The tray's (assets/tray): a disc in the phone's state colour with a white
  handset, as PNG (Linux, macOS) and ICO (Windows).
- The app's: the handset on a rounded square in Anvil's accent colour, for
  macOS (the AppIcon set), Windows (app_icon.ico), Linux
  (linux/packaging/anvil.png, the installers' icon) and Android (the
  launcher's mipmaps); on a full, opaque square for iOS (the AppIcon set),
  which rounds the corners itself and refuses an alpha channel."""

import math
import os
import struct
import zlib

STATES = {
    "ready": (0x1F, 0x9D, 0x55),     # AnvilColors.answer
    "offline": (0x9A, 0xA5, 0xB1),   # grey
    "dnd": (0xD6, 0x45, 0x45),       # AnvilColors.hangup
    "in_call": (0x0E, 0x8A, 0x7E),   # AnvilColors.accent
    "ringing": (0xD9, 0x9A, 0x1E),   # AnvilColors.held
}
ACCENT = (0x0E, 0x8A, 0x7E)  # AnvilColors.accent
SS = 4  # samples per pixel, each way


def handset(x, y):
    """Whether (x, y), in a unit square centred on 0, is on the handset: a
    thick arc with an earpiece and a mouthpiece at its ends."""
    # The arc's centre sits up and to the right; the handset curves round it.
    cx, cy = 0.22, -0.22
    d = math.hypot(x - cx, y - cy)
    a = math.atan2(y - cy, x - cx)
    on_arc = 0.30 <= d <= 0.46 and math.radians(90) <= a <= math.radians(180)
    ends = []
    for deg in (90, 180):
        r = math.radians(deg)
        ends.append((cx + 0.38 * math.cos(r), cy + 0.38 * math.sin(r)))
    on_end = any(math.hypot(x - ex, y - ey) <= 0.15 for ex, ey in ends)
    return on_arc or on_end


def disc(x, y):
    return math.hypot(x, y) <= 0.48, 1.6


def tile(x, y):
    """A rounded square with a margin, as macOS lays out an app icon."""
    half, radius = 0.40, 0.13
    dx, dy = max(abs(x) - (half - radius), 0), max(abs(y) - (half - radius), 0)
    return math.hypot(dx, dy) <= radius, 1.75


def square(x, y):
    """The whole square: iOS draws its own corners."""
    return True, 1.45


def render(size, colour, shape=disc, ss=SS, opaque=False):
    rows = []
    for py in range(size):
        row = bytearray([0])  # PNG filter: none
        for px in range(size):
            disc = white = 0
            for sy in range(ss):
                for sx in range(ss):
                    x = (px + (sx + 0.5) / ss) / size - 0.5
                    y = (py + (sy + 0.5) / ss) / size - 0.5
                    inside, scale = shape(x, y)
                    if inside:
                        disc += 1
                        # The handset, flipped so the mouthpiece is lower left.
                        if handset(x * scale, -y * scale):
                            white += 1
            n = ss * ss
            alpha = disc / n
            w = white / disc if disc else 0
            rgb = [round(c * (1 - w) + 255 * w) for c in colour]
            row += bytes(rgb if opaque else rgb + [round(alpha * 255)])
        rows.append(bytes(row))
    return png(size, b"".join(rows), opaque)


def png(size, raw, opaque=False):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    # Colour type 2 is RGB, 6 is RGBA.
    header = struct.pack(">IIBBBBB", size, size, 8, 2 if opaque else 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def ico(images):
    """An ICO holding PNG images (Windows Vista and later)."""
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    data = b""
    for size, image in images:
        out += struct.pack(
            "<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(image), offset + len(data)
        )
        data += image
    return out + data


def main():
    app = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
    here = os.path.join(app, "assets", "tray")
    os.makedirs(here, exist_ok=True)
    for name, colour in STATES.items():
        big = render(64, colour)
        with open(os.path.join(here, f"{name}.png"), "wb") as f:
            f.write(big)
        with open(os.path.join(here, f"{name}.ico"), "wb") as f:
            f.write(ico([(16, render(16, colour)), (32, render(32, colour)), (64, big)]))

    def app_icon(size):
        return render(size, ACCENT, tile, ss=4 if size <= 256 else 2)

    icons = {size: app_icon(size) for size in (16, 32, 48, 64, 128, 256, 512, 1024)}
    mac = os.path.join(app, "macos", "Runner", "Assets.xcassets", "AppIcon.appiconset")
    for size in (16, 32, 64, 128, 256, 512, 1024):
        with open(os.path.join(mac, f"app_icon_{size}.png"), "wb") as f:
            f.write(icons[size])
    with open(os.path.join(app, "windows", "runner", "resources", "app_icon.ico"), "wb") as f:
        f.write(ico([(s, icons[s]) for s in (16, 32, 48, 64, 128, 256)]))
    linux = os.path.join(app, "linux", "packaging")
    os.makedirs(linux, exist_ok=True)
    with open(os.path.join(linux, "anvil.png"), "wb") as f:
        f.write(icons[512])

    res = os.path.join(app, "android", "app", "src", "main", "res")
    for density, size in (("mdpi", 48), ("hdpi", 72), ("xhdpi", 96), ("xxhdpi", 144), ("xxxhdpi", 192)):
        with open(os.path.join(res, f"mipmap-{density}", "ic_launcher.png"), "wb") as f:
            f.write(app_icon(size))

    ios = os.path.join(app, "ios", "Runner", "Assets.xcassets", "AppIcon.appiconset")
    for name in sorted(os.listdir(ios)):
        if name.startswith("Icon-App-") and name.endswith(".png"):
            # Icon-App-83.5x83.5@2x.png: 83.5 points at 2x is 167 pixels.
            points, scale = name[len("Icon-App-"):-len(".png")].split("@")
            size = round(float(points.split("x")[0]) * int(scale[:-1]))
            with open(os.path.join(ios, name), "wb") as f:
                f.write(render(size, ACCENT, square, ss=4 if size <= 256 else 2, opaque=True))


if __name__ == "__main__":
    main()
