"""Draw the source icon, then: npm run tauri icon src-tauri/icons/source.png

A radio button with a stroke through it: the thing this app exists to undo.
Kept as code rather than an opaque binary so the shape can be adjusted.
"""

import struct
import zlib

SIZE = 1024
BACKGROUND = (0x25, 0x2A, 0x33)
RING = (0xE7, 0xE9, 0xEE)
STROKE = (0xF0, 0xA9, 0x4A)


def coverage(distance: float) -> float:
    """Antialias by reading a signed distance as partial pixel coverage."""
    return min(max(0.5 - distance, 0.0), 1.0)


def over(base, colour, alpha):
    return tuple(round(c * alpha + b * (1 - alpha)) for c, b in zip(colour, base))


def rounded_square(x, y, half, radius):
    dx = abs(x) - (half - radius)
    dy = abs(y) - (half - radius)
    outside = (max(dx, 0.0) ** 2 + max(dy, 0.0) ** 2) ** 0.5
    return outside + min(max(dx, dy), 0.0) - radius


def build() -> bytes:
    rows = []
    centre = SIZE / 2
    for py in range(SIZE):
        row = bytearray()
        y = py + 0.5 - centre
        for px in range(SIZE):
            x = px + 0.5 - centre
            pixel = (0, 0, 0)
            alpha = 0.0

            plate = coverage(rounded_square(x, y, SIZE * 0.5, SIZE * 0.19))
            if plate:
                pixel, alpha = BACKGROUND, plate

            # The radio button outline.
            radius = (x * x + y * y) ** 0.5
            ring = coverage(abs(radius - SIZE * 0.25) - SIZE * 0.045)
            if ring:
                pixel = over(pixel, RING, ring)
                alpha = max(alpha, ring)

            # A stroke through it, trimmed to just past the ring.
            bar = coverage(abs(x + y) / 2**0.5 - SIZE * 0.035)
            bar = min(bar, coverage(radius - SIZE * 0.34))
            if bar:
                pixel = over(pixel, STROKE, bar)
                alpha = max(alpha, bar)

            row += bytes((*pixel, round(alpha * 255)))
        rows.append(bytes(row))

    raw = b"".join(b"\x00" + row for row in rows)

    def chunk(kind: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


if __name__ == "__main__":
    with open("source.png", "wb") as handle:
        handle.write(build())
    print(f"wrote source.png ({SIZE}x{SIZE})")
