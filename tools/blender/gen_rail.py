"""gen_rail.py — 3-car commuter/rail consist, heavier than the metro.

Run:  blender -b --factory-startup --python gen_rail.py -- <out.glb>
Add   --preview <prefix>  to render a turntable critique sheet.

Distinct from gen_train.py (metro): HEAVIER profile (taller/wider bodies, a
deeper underframe skirt), FEWER doors (1 per car side vs metro's 2), and a
subtle ROOF EQUIPMENT hint (a low pantograph/AC block on each car) so it reads
as a mainline train rather than a metro.

Silhouette checklist (score from the turntable BEFORE trusting the .glb):
  * SIDE : 3 heavy cars, one door per car side (fewer than metro), a window
           band that reads but is broken up more, a deep dark underframe skirt,
           a small roof equipment block per car.
  * FRONT: a tall blunt cab with a dark screen + a roof block silhouette.
  * <2000 tris per car.

Orientation: consist runs along +X, base at z=0. BODY near-white
('transit_body') for per-route tint; windows/doors/roof-gear neutral dark,
roof cap neutral grey.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

CARS = 3
CAR_LEN = 11.0       # longer than metro's 9m
GAP = 0.7
WIDTH = 3.9          # wider/heavier than metro's 3.6
HEIGHT = 4.0         # taller than metro's 3.6
FLOOR = 1.1          # DEEP underframe skirt (heavy mainline read)
ROOF_RISE = 0.45
DOORS = 1            # FEWER doors than metro (metro has 2)


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
    verts, faces = [], []
    top = HEIGHT - ROOF_RISE
    for cx in _car_centers():
        _append(verts, faces,
                _box(cx, 0, top + ROOF_RISE / 2, CAR_LEN * 0.9, WIDTH * 0.84, ROOF_RISE))
    return mf.new_mesh_object("roof", verts, faces, mat)


def build_roof_gear(mat):
    """Subtle roof equipment: a low pantograph/AC block on each car roof (the
    catenary-free 'keep subtle' hint from the brief)."""
    verts, faces = [], []
    top = HEIGHT
    for cx in _car_centers():
        _append(verts, faces, _box(cx, 0, top + 0.22, CAR_LEN * 0.28, WIDTH * 0.5, 0.44))
    return mf.new_mesh_object("roof_gear", verts, faces, mat)


def build_glazing(mat):
    """Window band, but broken into per-car panes (heavier, fewer-glass read)
    plus cab screens on the outer ends."""
    verts, faces = [], []
    centers = _car_centers()
    band_z = WIDTH / 2 + 0.02
    band_cy = FLOOR + (HEIGHT - ROOF_RISE - FLOOR) * 0.60
    for cx in centers:
        for side in (-1, 1):
            # two shorter panes per car (broken band) rather than one long band
            for seg in (-1, 1):
                sx = cx + seg * CAR_LEN * 0.24
                _append(verts, faces, _box(sx, side * band_z, band_cy, CAR_LEN * 0.34, 0.05, 0.95))
    ends = [(centers[0], -1), (centers[-1], 1)]
    for cx, sgn in ends:
        ex = cx + sgn * (CAR_LEN / 2 * 0.98)
        _append(verts, faces, _box(ex, 0, HEIGHT * 0.62, 0.05, WIDTH * 0.78, 1.2))
    return mf.new_mesh_object("glazing", verts, faces, mat)


def build_doors(mat):
    """One door per car side (fewer than metro)."""
    verts, faces = [], []
    door_z = WIDTH / 2 + 0.03
    door_h = (HEIGHT - ROOF_RISE) - FLOOR - 0.18
    for cx in _car_centers():
        for side in (-1, 1):
            _append(verts, faces, _box(cx, side * door_z, FLOOR + door_h / 2, 1.0, 0.05, door_h))
    return mf.new_mesh_object("doors", verts, faces, mat)


def build_underframe(mat):
    """Deep dark underframe skirt + trucks below FLOOR — the heavy mainline
    running gear read."""
    verts, faces = [], []
    for cx in _car_centers():
        # continuous skirt band
        _append(verts, faces, _box(cx, 0, FLOOR * 0.72, CAR_LEN * 0.92, WIDTH * 0.9, FLOOR * 0.5))
        # two bogies
        for end in (-1, 1):
            bx = cx + end * CAR_LEN * 0.32
            _append(verts, faces, _box(bx, 0, FLOOR * 0.3, 2.7, WIDTH * 0.78, FLOOR * 0.6))
    return mf.new_mesh_object("underframe", verts, faces, mat)


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
        out = "/tmp/rail.glb"

    mf.reset_scene()
    m_body = mf.palette_material("transit_body", "transit_body")
    m_glass = mf.palette_material("transit_glass", "transit_glass")
    m_roof = mf.palette_material("transit_roof", "transit_roof")

    build_bodies(m_body)
    build_roof(m_roof)
    build_roof_gear(m_glass)
    build_glazing(m_glass)
    build_doors(m_glass)
    build_underframe(m_glass)

    if preview:
        import turntable
        turntable.render_turntable(preview)
    if out:
        mf.export_glb(out)


main()
