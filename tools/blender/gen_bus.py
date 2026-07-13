"""gen_bus.py — single-unit boxy transit bus, polished under the model-craft loop.

Run:  blender -b --factory-startup --python gen_bus.py -- <out.glb>
Add   --preview <prefix>  to render a turntable critique sheet.

Silhouette checklist (score each from the turntable BEFORE trusting the .glb):
  * SIDE  : boxy body, continuous window band, curbside door pattern (2 doors
            on the +Y / curb side only), wheels reading below a lifted body.
  * FRONT : a wrap windshield reads dark; slight roof radius (narrower raised
            roof slab) so the top isn't a hard brick edge.
  * ROOF  : neutral roof cap, subtly narrower than the body.
  * <2000 tris.

Orientation: bus runs along +X, base at z=0 (Bevy raises it onto the deck).
BODY material is near-white ('transit_body') so Bevy's per-route base_color
tint reads through; windows/doors neutral dark, roof neutral grey.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

LEN = 12.0
WIDTH = 2.55
HEIGHT = 3.15       # roofline above rail
FLOOR = 0.55        # underframe height: wheels below, body above
ROOF_RISE = 0.28
DOORS = 2           # curbside doors
CURB = 1            # +Y side is the curb (doors go here)


def _box(cx, cy, cz, sx, sy, sz):
    hx, hy, hz = sx / 2, sy / 2, sz / 2
    v = [
        (cx - hx, cy - hy, cz - hz), (cx + hx, cy - hy, cz - hz),
        (cx + hx, cy + hy, cz - hz), (cx - hx, cy + hy, cz - hz),
        (cx - hx, cy - hy, cz + hz), (cx + hx, cy - hy, cz + hz),
        (cx + hx, cy + hy, cz + hz), (cx - hx, cy + hy, cz + hz),
    ]
    f = [(0, 1, 2, 3), (4, 7, 6, 5), (0, 4, 5, 1),
         (1, 5, 6, 2), (2, 6, 7, 3), (3, 7, 4, 0)]
    return v, f


def _append(verts, faces, box):
    bv, bf = box
    off = len(verts)
    verts.extend(bv)
    faces.extend([tuple(i + off for i in fc) for fc in bf])


def build_body(mat):
    verts, faces = [], []
    top = HEIGHT - ROOF_RISE
    body_h = top - FLOOR
    _append(verts, faces, _box(0, 0, (FLOOR + top) / 2, LEN, WIDTH, body_h))
    return mf.new_mesh_object("body", verts, faces, mat)


def build_roof(mat):
    """Narrower raised roof slab -> rounded silhouette / roof radius read."""
    verts, faces = [], []
    top = HEIGHT - ROOF_RISE
    _append(verts, faces, _box(0, 0, top + ROOF_RISE / 2, LEN * 0.92, WIDTH * 0.8, ROOF_RISE))
    return mf.new_mesh_object("roof", verts, faces, mat)


def build_glazing(mat):
    """Continuous side window band on BOTH sides + a wrap windshield on the
    front (+X) end so the front view reads a dark screen."""
    verts, faces = [], []
    band_z = WIDTH / 2 + 0.02
    band_cy = FLOOR + (HEIGHT - ROOF_RISE - FLOOR) * 0.60
    for side in (-1, 1):
        _append(verts, faces, _box(0, side * band_z, band_cy, LEN * 0.9, 0.05, 1.05))
    # windshield: dark screen sitting ON the front face (just proud of it so it
    # is never occluded by the body's own front quad), in the upper body band.
    fx = LEN / 2 + 0.03
    wind_cy = FLOOR + (HEIGHT - ROOF_RISE - FLOOR) * 0.70
    _append(verts, faces, _box(fx, 0, wind_cy, 0.06, WIDTH * 0.84, 1.15))
    return mf.new_mesh_object("glazing", verts, faces, mat)


def build_doors(mat):
    """Curbside door pattern: DOORS tall slots on the +Y side, running from the
    window band down toward the floor (taller than the windows)."""
    verts, faces = [], []
    door_y = CURB * (WIDTH / 2 + 0.03)
    door_h = (HEIGHT - ROOF_RISE) - FLOOR - 0.12
    for d in range(DOORS):
        frac = (d + 1) / (DOORS + 1)
        dx = (frac - 0.5) * LEN * 0.82
        _append(verts, faces, _box(dx, door_y, FLOOR + door_h / 2, 1.0, 0.05, door_h))
    return mf.new_mesh_object("doors", verts, faces, mat)


def build_wheels(mat):
    """Two axle blocks in the underframe zone (below FLOOR) so the running gear
    reads dark under a lifted white body."""
    verts, faces = [], []
    for end in (-1, 1):
        wx = end * LEN * 0.32
        _append(verts, faces, _box(wx, 0, FLOOR / 2, 1.7, WIDTH * 0.92, FLOOR))
    return mf.new_mesh_object("wheels", verts, faces, mat)


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    out = None
    preview = None
    i = 0
    while i < len(argv):
        if argv[i] == "--preview":
            preview = argv[i + 1]; i += 2
        else:
            out = argv[i]; i += 1
    if out is None:
        out = "/tmp/bus.glb"

    mf.reset_scene()
    m_body = mf.palette_material("transit_body", "transit_body")
    m_glass = mf.palette_material("transit_glass", "transit_glass")
    m_roof = mf.palette_material("transit_roof", "transit_roof")

    build_body(m_body)
    build_roof(m_roof)
    build_glazing(m_glass)
    build_doors(m_glass)
    build_wheels(m_glass)

    if preview:
        import turntable
        turntable.render_turntable(preview)
    if out:
        mf.export_glb(out)


main()
