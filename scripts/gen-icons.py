#!/usr/bin/env python3
"""Regenerate packaging/icon.{png,ico,icns} from the app icon geometry.

Mirrors crates/mf-game/src/app_icon.rs::render_icon_rgba (dark rounded
badge, four colored corner spokes, ringed hub) at 512px. The generated
binaries are committed so CI does not need Python/Pillow; rerun this only
when the logo geometry changes, and keep it in lockstep with app_icon.rs
and hud.rs::draw_logo.

Requires: pip install Pillow
"""
from PIL import Image, ImageDraw


def render(size: int, ss: int = 4) -> Image.Image:
    S = size * ss
    s = float(S)
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    bg = (0x0B, 0x0D, 0x10, 255)
    d.rounded_rectangle([0, 0, S - 1, S - 1], radius=s * 0.22, fill=bg)
    cx = cy = s / 2
    arm = s * 0.35
    hw = s * 0.05
    corners = [
        ((0x7E, 0xF2, 0x9A, 255), (-1, -1)),
        ((0x54, 0xD0, 0xFF, 255), (1, -1)),
        ((0xFF, 0xB6, 0x3D, 255), (1, 1)),
        ((0xFF, 0x5D, 0x6C, 255), (-1, 1)),
    ]
    for color, (dx, dy) in corners:
        ex, ey = cx + dx * arm, cy + dy * arm
        d.line([cx, cy, ex, ey], fill=color, width=int(2 * hw))
        d.ellipse([ex - hw, ey - hw, ex + hw, ey + hw], fill=color)
    hub_r = s * 0.15
    inner = hub_r - s * 0.075
    d.ellipse([cx - hub_r, cy - hub_r, cx + hub_r, cy + hub_r], fill=(0xF4, 0xF4, 0xF5, 255))
    d.ellipse([cx - inner, cy - inner, cx + inner, cy + inner], fill=bg)
    return img.resize((size, size), Image.LANCZOS)


if __name__ == "__main__":
    base = render(512)
    sizes = [(n, n) for n in (16, 24, 32, 48, 64, 128, 256)]
    base.save("packaging/icon.png")
    base.save("packaging/icon.ico", sizes=sizes)
    base.save("packaging/icon.icns")
    print("wrote packaging/icon.{png,ico,icns}")
