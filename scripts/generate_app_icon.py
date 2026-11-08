#!/usr/bin/env python3
"""Generate VoicePort's original microphone icon with the Python standard library.

Run explicitly on macOS; packaging installs the committed .icns without regeneration.
"""
import pathlib
import struct
import subprocess
import tempfile
import zlib

ROOT = pathlib.Path(__file__).resolve().parents[1]
SIZE = 1024


def rounded(x, y, left, top, right, bottom, radius):
    cx = max(left + radius, min(x, right - radius))
    cy = max(top + radius, min(y, bottom - radius))
    return (x - cx) ** 2 + (y - cy) ** 2 <= radius ** 2


def pixel(x, y):
    if not rounded(x, y, 64, 64, 960, 960, 200):
        return (0, 0, 0, 0)
    microphone = rounded(x, y, 432, 230, 592, 590, 80)
    distance = (x - 512) ** 2 + (y - 526) ** 2
    cradle = (y >= 526 and 136 ** 2 <= distance <= 164 ** 2)
    cradle |= 348 <= x <= 376 and 450 <= y <= 526
    cradle |= 648 <= x <= 676 and 450 <= y <= 526
    stem = 498 <= x <= 526 and 688 <= y <= 758
    base = rounded(x, y, 428, 748, 596, 776, 14)
    return (255, 255, 255, 255) if microphone or cradle or stem or base else (37, 99, 235, 255)


def chunk(kind, data):
    payload = kind + data
    return struct.pack('>I', len(data)) + payload + struct.pack('>I', zlib.crc32(payload))


def png():
    rows = bytearray()
    for y in range(SIZE):
        rows.append(0)
        for x in range(SIZE):
            samples = [pixel(x + dx, y + dy) for dx, dy in ((.25, .25), (.75, .25), (.25, .75), (.75, .75))]
            alpha = sum(p[3] for p in samples)
            # Average premultiplied samples, then return straight RGBA.
            rgb = [round(sum(p[c] * p[3] for p in samples) / alpha) if alpha else 0 for c in range(3)]
            rows.extend((*rgb, round(alpha / 4)))
    header = struct.pack('>IIBBBBB', SIZE, SIZE, 8, 6, 0, 0, 0)
    return b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', header) + chunk(b'IDAT', zlib.compress(rows, 9)) + chunk(b'IEND', b'')


def main():
    with tempfile.TemporaryDirectory(prefix='voiceport-icon-') as temporary:
        directory = pathlib.Path(temporary)
        source = directory / 'source.png'
        source.write_bytes(png())
        iconset = directory / 'AppIcon.iconset'
        iconset.mkdir()
        for points in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                size = points * scale
                suffix = '@2x' if scale == 2 else ''
                target = iconset / f'icon_{points}x{points}{suffix}.png'
                subprocess.run(['sips', '-z', str(size), str(size), str(source), '--out', str(target)],
                               check=True, capture_output=True)
        subprocess.run(['iconutil', '-c', 'icns', str(iconset), '-o', str(ROOT / 'resources/AppIcon.icns')], check=True)
        print(ROOT / 'resources/AppIcon.icns')


if __name__ == '__main__':
    main()
