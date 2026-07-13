"""gen_train.py — metro consist (Pilot B). 3 cars, window band, curved roof,
bogie hint. Matches 'vehicle silhouettes v2'.

Run:  blender --background --python gen_train.py -- <out.glb>

Orientation: consist runs along +X (matches vehicles.rs box mesh, which is
positioned then rotated by heading about Y). Body centered at origin, sitting
so its base is at y=0 (Bevy raises the whole thing +3.0 like the brick).
The BODY material is 'transit_body' (near-white) so Bevy's per-route
StandardMaterial tint (base_color) reads through — the loader tints the body
submesh exactly like it tints the brick cuboid. Windows + roof are separate
materials that stay neutral (not tinted).

Base length is normalised to vehicles.rs VEHICLE_BASE_LENGTH * 3 cars so the
loader can scale it to the same footprint the brick pool uses.
"""

from __future__ import annotations

import sys
import os

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

CARS = 3
CAR_LEN = 9.0
GAP = 0.8
WIDTH = 3.6
HEIGHT = 3.4
ROOF_RISE = 0.5     # curve of roof above the box top


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
    faces.extend([tuple(i + off for i in face) for face in bf])


def build_bodies(mat):
    """3 car bodies with a slight roof curve (chamfered top)."""
    verts, faces = [], []
    total = CARS * CAR_LEN + (CARS - 1) * GAP
    x0 = -total / 2
    body_h = HEIGHT - ROOF_RISE
    for c in range(CARS):
        cx = x0 + CAR_LEN / 2 + c * (CAR_LEN + GAP)
        # main box (base at y=0)
        _append(verts, faces, _box(cx, 0, body_h / 2, CAR_LEN * 0.98, WIDTH, body_h))
        # curved roof: narrower top slab lifted, giving a rounded silhouette
        _append(verts, faces,
                _box(cx, 0, body_h + ROOF_RISE / 2, CAR_LEN * 0.9, WIDTH * 0.82, ROOF_RISE))
    return mf.new_mesh_object("bodies", verts, faces, mat)


def build_windows(mat):
    """Continuous window band along each side of each car."""
    verts, faces = [], []
    total = CARS * CAR_LEN + (CARS - 1) * GAP
    x0 = -total / 2
    band_z = WIDTH / 2 + 0.02
    band_cy = HEIGHT * 0.62
    for c in range(CARS):
        cx = x0 + CAR_LEN / 2 + c * (CAR_LEN + GAP)
        for side in (-1, 1):
            _append(verts, faces,
                    _box(cx, side * band_z, band_cy, CAR_LEN * 0.8, 0.06, 1.0))
    return mf.new_mesh_object("windows", verts, faces, mat)


def build_bogies(mat):
    """Bogie hint: small dark boxes under each car end."""
    verts, faces = [], []
    total = CARS * CAR_LEN + (CARS - 1) * GAP
    x0 = -total / 2
    for c in range(CARS):
        cx = x0 + CAR_LEN / 2 + c * (CAR_LEN + GAP)
        for end in (-1, 1):
            bx = cx + end * CAR_LEN * 0.32
            _append(verts, faces, _box(bx, 0, 0.35, 1.8, WIDTH * 0.7, 0.7))
    return mf.new_mesh_object("bogies", verts, faces, mat)


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    out = argv[0] if argv else "/tmp/train_metro.glb"

    mf.reset_scene()
    m_body = mf.palette_material("transit_body", "transit_body")
    m_glass = mf.palette_material("transit_glass", "transit_glass")
    m_roof = mf.palette_material("transit_roof", "transit_roof")

    build_bodies(m_body)
    build_windows(m_glass)
    build_bogies(m_glass)
    # roof accent as its own object keeps its neutral tint under a body mesh join
    build_bogies  # noqa

    mf.export_glb(out)


main()
