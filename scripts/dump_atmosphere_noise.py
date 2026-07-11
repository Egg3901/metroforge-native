#!/usr/bin/env python3
"""Dump atmosphere noise diagnostics matching mf-render/src/atmosphere.rs.

Writes:
  cloud_shadow_tile.png  — sparse 2D ground-shadow noise (mostly clear)
  cloud_blob_mid_slice.png — soft FogVolume density mid-slice (empty edges)
  old_slab_fill.png — contrast: old filled-slab look (for PR justification)
"""

from __future__ import annotations

import math
import struct
import zlib
from pathlib import Path


def hash21(x: int, y: int) -> float:
    n = (x * 374761393 + y * 668265263) & 0xFFFFFFFF
    if n >= 0x80000000:
        n -= 0x100000000
    n = ((n ^ (n >> 13)) * 1274126177) & 0xFFFFFFFF
    if n >= 0x80000000:
        n -= 0x100000000
    return (n & 0xFFFF) / 65535.0


def hash31(x: int, y: int, z: int) -> float:
    n = (x * 374761393 + y * 668265263 + z * 1274126177) & 0xFFFFFFFF
    if n >= 0x80000000:
        n -= 0x100000000
    n = ((n ^ (n >> 13)) * 1274126177) & 0xFFFFFFFF
    if n >= 0x80000000:
        n -= 0x100000000
    return (n & 0xFFFF) / 65535.0


def value_noise_2(x: float, y: float) -> float:
    x0, y0 = math.floor(x), math.floor(y)
    fx, fy = x - x0, y - y0
    ux = fx * fx * (3 - 2 * fx)
    uy = fy * fy * (3 - 2 * fy)
    c00 = hash21(x0, y0)
    c10 = hash21(x0 + 1, y0)
    c01 = hash21(x0, y0 + 1)
    c11 = hash21(x0 + 1, y0 + 1)
    x0v = c00 + (c10 - c00) * ux
    x1v = c01 + (c11 - c01) * ux
    return x0v + (x1v - x0v) * uy


def value_noise_3(x: float, y: float, z: float) -> float:
    x0, y0, z0 = math.floor(x), math.floor(y), math.floor(z)
    fx, fy, fz = x - x0, y - y0, z - z0
    ux = fx * fx * (3 - 2 * fx)
    uy = fy * fy * (3 - 2 * fy)
    uz = fz * fz * (3 - 2 * fz)

    def h(dx, dy, dz):
        return hash31(x0 + dx, y0 + dy, z0 + dz)

    x00 = h(0, 0, 0) + (h(1, 0, 0) - h(0, 0, 0)) * ux
    x10 = h(0, 1, 0) + (h(1, 1, 0) - h(0, 1, 0)) * ux
    x01 = h(0, 0, 1) + (h(1, 0, 1) - h(0, 0, 1)) * ux
    x11 = h(0, 1, 1) + (h(1, 1, 1) - h(0, 1, 1)) * ux
    y0v = x00 + (x10 - x00) * uy
    y1v = x01 + (x11 - x01) * uy
    return y0v + (y1v - y0v) * uz


def fbm2(x: float, y: float, octaves: int) -> float:
    amp, freq, s, n = 0.5, 1.0, 0.0, 0.0
    for _ in range(octaves):
        s += amp * value_noise_2(x * freq, y * freq)
        n += amp
        amp *= 0.5
        freq *= 2.03
    return s / max(n, 1e-4)


def fbm3(x: float, y: float, z: float, octaves: int) -> float:
    amp, freq, s, n = 0.5, 1.0, 0.0, 0.0
    for _ in range(octaves):
        s += amp * value_noise_3(x * freq, y * freq, z * freq)
        n += amp
        amp *= 0.5
        freq *= 2.03
    return s / max(n, 1e-4)


def write_png(path: Path, w: int, h: int, rgb: bytes) -> None:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + tag + data + struct.pack(
            ">I", zlib.crc32(tag + data) & 0xFFFFFFFF
        )

    raw = bytearray()
    for y in range(h):
        raw.append(0)
        raw.extend(rgb[y * w * 3 : (y + 1) * w * 3])
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(
        b"IDAT", zlib.compress(bytes(raw), 9)
    ) + chunk(b"IEND", b"")
    path.write_bytes(png)


def shadow_tile(n: int = 256) -> bytes:
    out = bytearray(n * n * 3)
    for y in range(n):
        for x in range(n):
            u, v = x / n, y / n
            a = fbm2(u * 2.2, v * 2.2, 4)
            b = fbm2(u * 3.5 + 5.1, v * 3.5 + 2.3, 3)
            dens = max(0.0, min(1.0, a * 0.65 + b * 0.35))
            shaped = (max(0.0, dens - 0.48) / 0.52) ** 1.6
            g = int(round(shaped * 255))
            i = (y * n + x) * 3
            out[i : i + 3] = bytes((g, g, g))
    return bytes(out)


def blob_slice(n: int = 64) -> bytes:
    out = bytearray(n * n * 3)
    y = n // 2
    for z in range(n):
        for x in range(n):
            u, v, w = x / n, y / n, z / n
            dx, dy, dz = (u - 0.5) * 2.0, (v - 0.5) * 2.6, (w - 0.5) * 2.0
            r = math.sqrt(dx * dx + dy * dy + dz * dz)
            warp = fbm3(u * 3.0 + 1.7, v * 2.0, w * 3.0 + 0.4, 3) * 0.22
            soft = (1.0 - max(0.0, min(1.0, (r + warp - 0.15) / 0.85))) ** 1.8
            shaped = 0.0 if soft < 0.08 else ((soft - 0.08) / 0.92) ** 1.35
            g = int(round(shaped * 255))
            i = (z * n + x) * 3
            out[i : i + 3] = bytes((g, g, g))
    return bytes(out)


def old_slab_fill(n: int = 256) -> bytes:
    """Approximate the old filled mist/cloud slab — high fill fraction."""
    out = bytearray(n * n * 3)
    for y in range(n):
        for x in range(n):
            u, v = x / n, y / n
            fbm = fbm2(u * 2.5, v * 2.5, 4)
            shaped = max(0.0, min(1.0, fbm * 1.15)) ** 1.15
            g = int(round(shaped * 255))
            i = (y * n + x) * 3
            out[i : i + 3] = bytes((g, g, g))
    return bytes(out)


def main() -> None:
    out = Path("/opt/cursor/artifacts/atmosphere")
    out.mkdir(parents=True, exist_ok=True)
    write_png(out / "cloud_shadow_tile.png", 256, 256, shadow_tile(256))
    write_png(out / "cloud_blob_mid_slice.png", 64, 64, blob_slice(64))
    write_png(out / "old_slab_fill.png", 256, 256, old_slab_fill(256))
    print(f"wrote diagnostics to {out}")


if __name__ == "__main__":
    main()
