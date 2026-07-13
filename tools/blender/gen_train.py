"""gen_train.py — metro consist (3 cars), polished under the model-craft loop.

Run:  blender -b --factory-startup --python gen_train.py -- <out.glb>
Add   --preview <prefix>  to render a turntable critique sheet.

Polish targets (silhouette checklist):
  * FRONT view: a cab window on the lead car + a clear door pattern.
  * SIDE view: a continuous window band reading across all 3 cars, with door
    slots interrupting it, and bogies clearly visible under each car.
  * <2000 tris per car.

Orientation: consist runs along +X, base at z=0 (Bevy raises it onto the deck).
BODY material is near-white ('transit_body') so Bevy's per-route base_color tint
reads through; windows / doors / bogies stay neutral dark, roof stays neutral.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

CARS = 3
CAR_LEN = 9.0
GAP = 0.8
WIDTH = 3.6
HEIGHT = 3.6        # roofline height above rail
FLOOR = 0.85        # underframe height: bogies live below this, body above
ROOF_RISE = 0.5
DOORS = 2            # doors per car side


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


def _car_centers():
    total = CARS * CAR_LEN + (CARS - 1) * GAP
    x0 = -total / 2
    return [x0 + CAR_LEN / 2 + c * (CAR_LEN + GAP) for c in range(CARS)]


def build_bodies(mat):
    verts, faces = [], []
    top = HEIGHT - ROOF_RISE
    body_h = top - FLOOR
    for cx in _car_centers():
        _append(verts, faces, _box(cx, 0, (FLOOR + top) / 2, CAR_LEN * 0.98, WIDTH, body_h))
    return mf.new_mesh_object("bodies", verts, faces, mat)


def build_roof(mat):
    """Slightly narrower raised roof slab per car -> rounded silhouette."""
    verts, faces = [], []
    top = HEIGHT - ROOF_RISE
    for cx in _car_centers():
        _append(verts, faces,
                _box(cx, 0, top + ROOF_RISE / 2, CAR_LEN * 0.9, WIDTH * 0.82, ROOF_RISE))
    return mf.new_mesh_object("roof", verts, faces, mat)


def build_glazing(mat):
    """Continuous side window band + cab windows on the lead/tail ends. The
    band runs nearly the full car length on both sides so it reads continuous
    across the consist."""
    verts, faces = [], []
    centers = _car_centers()
    band_z = WIDTH / 2 + 0.02
    band_cy = FLOOR + (HEIGHT - ROOF_RISE - FLOOR) * 0.62
    for cx in centers:
        for side in (-1, 1):
            _append(verts, faces,
                    _box(cx, side * band_z, band_cy, CAR_LEN * 0.86, 0.06, 1.05))
    # cab windows: wrap-around dark screen on the two outward end faces
    ends = [(centers[0], -1, CAR_LEN / 2), (centers[-1], 1, CAR_LEN / 2)]
    for cx, sgn, half in ends:
        ex = cx + sgn * (half * 0.98)
        _append(verts, faces, _box(ex, 0, HEIGHT * 0.66, 0.06, WIDTH * 0.78, 1.15))
    return mf.new_mesh_object("glazing", verts, faces, mat)


def build_doors(mat):
    """Vertical door slots interrupting the window band, DOORS per car side.
    They run from the band down toward the floor so they read taller than the
    windows (the door pattern the front/side checklist calls for)."""
    verts, faces = [], []
    door_z = WIDTH / 2 + 0.03
    for cx in _car_centers():
        for d in range(DOORS):
            frac = (d + 1) / (DOORS + 1)
            dx = cx + (frac - 0.5) * CAR_LEN * 0.9
            door_h = (HEIGHT - ROOF_RISE) - FLOOR - 0.2
            for side in (-1, 1):
                _append(verts, faces,
                        _box(dx, side * door_z, FLOOR + door_h / 2, 0.9, 0.05, door_h))
    return mf.new_mesh_object("doors", verts, faces, mat)


def build_bogies(mat):
    """Two bogies per car in the underframe zone (below FLOOR) so the running
    gear reads as dark trucks under a lifted white body."""
    verts, faces = [], []
    for cx in _car_centers():
        for end in (-1, 1):
            bx = cx + end * CAR_LEN * 0.30
            _append(verts, faces, _box(bx, 0, FLOOR / 2, 2.4, WIDTH * 0.7, FLOOR))
    return mf.new_mesh_object("bogies", verts, faces, mat)


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
        out = "/tmp/train_metro.glb"

    mf.reset_scene()
    m_body = mf.palette_material("transit_body", "transit_body")
    m_glass = mf.palette_material("transit_glass", "transit_glass")
    m_roof = mf.palette_material("transit_roof", "transit_roof")

    build_bodies(m_body)
    build_roof(m_roof)
    build_glazing(m_glass)
    build_doors(m_glass)
    build_bogies(m_glass)

    if preview:
        import turntable
        turntable.render_turntable(preview)
    if out:
        mf.export_glb(out)


main()
