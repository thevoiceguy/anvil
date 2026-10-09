#!/usr/bin/env python3
"""The tray's icons (assets/tray): a disc in the phone's state colour with a
white handset, as PNG (Linux, macOS) and ICO (Windows). Pure Python, so
anyone can run it again: `python3 tool/tray_icons.py`."""

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


def render(size, colour):
    rows = []
    for py in range(size):
        row = bytearray([0])  # PNG filter: none
        for px in range(size):
            disc = white = 0
            for sy in range(SS):
                for sx in range(SS):
                    x = (px + (sx + 0.5) / SS) / size - 0.5
                    y = (py + (sy + 0.5) / SS) / size - 0.5
                    if math.hypot(x, y) <= 0.48:
                        disc += 1
                        # The handset, flipped so the mouthpiece is lower left.
                        if handset(x * 1.6, -y * 1.6):
                            white += 1
            n = SS * SS
            alpha = disc / n
            w = white / disc if disc else 0
            rgb = [round(c * (1 - w) + 255 * w) for c in colour]
            row += bytes(rgb + [round(alpha * 255)])
        rows.append(bytes(row))
    return png(size, b"".join(rows))


def png(size, raw):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
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
    here = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets", "tray")
    os.makedirs(here, exist_ok=True)
    for name, colour in STATES.items():
        big = render(64, colour)
        with open(os.path.join(here, f"{name}.png"), "wb") as f:
            f.write(big)
        with open(os.path.join(here, f"{name}.ico"), "wb") as f:
            f.write(ico([(16, render(16, colour)), (32, render(32, colour)), (64, big)]))


if __name__ == "__main__":
    main()
