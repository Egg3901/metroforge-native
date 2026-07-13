"""gen_rail_viaduct.py — generic tileable elevated RAIL viaduct segment (kit).

Run:  blender -b --factory-startup --python gen_rail_viaduct.py -- <out.glb>
Add   --preview <prefix>  to render a turntable critique sheet.

The transit sibling of gen_viaduct.py: a NARROWER black deck with a raised
grey BALLAST / track-bed hint down the middle, carried on a white steel-style
TRESTLE pier (two slim columns + a transverse cap beam + an X cross-brace) so
elevated metro/rail reads distinct from the road's single T-pier viaduct.
Tiles end-to-end along an elevated TRANSIT track run (transit.rs); scaled Z to
the mode's track width, vertical-scaled so the trestle foot meets ground.

Authoring frame (Blender Z-up, exported +Y up):
  * +X run direction; deck TOP at z = 0; width along Y.

Poly budget ~600 tris/segment.
"""

from __future__ import annotations

import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

SEG_LEN = 20.0
OVERLAP = 0.4
DECK_W = 9.0
DECK_TH = 1.4
BALLAST_W = 4.4
BALLAST_H = 0.55
PIER_H = 9.0
COL_HALF = 3.0       # column offset from center in Y (trestle spread)
COL_W = 1.2          # column footprint
CAP_H = 1.1


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


def _append(verts, faces, geo):
    bv, bf = geo
    off = len(verts)
    verts.extend(bv)
    faces.extend([tuple(i + off for i in fc) for fc in bf])


def _beam_yz(verts, faces, a, b, w):
    """Square beam between a,b in the Y-Z plane (X fixed)."""
    ax, ay, az = a
    bx, by, bz = b
    mx, my, mz = ax, (ay + by) / 2, (az + bz) / 2
    d1, d2 = by - ay, bz - az
    length = math.hypot(d1, d2) + w
    v, f = _box(mx, my, mz, w, length, w)
    ang = math.atan2(d2, d1); ca, sa = math.cos(ang), math.sin(ang)
    rot = [(x, my + (y - my) * ca - (z - mz) * sa, mz + (y - my) * sa + (z - mz) * ca)
           for (x, y, z) in v]
    off = len(verts)
    verts.extend(rot)
    faces.extend([tuple(i + off for i in fc) for fc in f])


def build_deck(mat):
    verts, faces = [], []
    length = SEG_LEN + OVERLAP
    _append(verts, faces, _box(0.0, 0.0, -DECK_TH / 2, length, DECK_W, DECK_TH))
    return mf.new_mesh_object("rail_viaduct_deck", verts, faces, mat)


def build_ballast(mat):
    """Raised grey ballast bed hint down the deck centerline + slim parapet
    curbs at the edges (the elevated-rail read)."""
    verts, faces = [], []
    length = SEG_LEN + OVERLAP
    _append(verts, faces, _box(0.0, 0.0, BALLAST_H / 2, length, BALLAST_W, BALLAST_H))
    half = DECK_W / 2
    for s in (-1.0, 1.0):
        _append(verts, faces, _box(0.0, s * (half - 0.3), 0.3, length, 0.5, 0.6))
    return mf.new_mesh_object("rail_viaduct_ballast", verts, faces, mat)


def build_rails(mat):
    """Two thin dark rail lines riding the ballast — the track-bed read."""
    verts, faces = [], []
    length = SEG_LEN + OVERLAP
    ztop = BALLAST_H + 0.18
    for s in (-1.0, 1.0):
        _append(verts, faces, _box(0.0, s * 0.9, ztop - 0.18, length, 0.26, 0.36))
    return mf.new_mesh_object("rail_viaduct_rails", verts, faces, mat)


def build_trestle(mat):
    verts, faces = [], []
    col_top = -DECK_TH - CAP_H
    foot = col_top - PIER_H
    # Transverse cap beam under the deck spanning the two columns.
    _append(verts, faces, _box(0.0, 0.0, -DECK_TH - CAP_H / 2,
                               COL_W + 0.4, COL_HALF * 2 + COL_W + 0.4, CAP_H))
    # Two columns.
    for s in (-1.0, 1.0):
        cy = s * COL_HALF
        _append(verts, faces, _box(0.0, cy, col_top - PIER_H / 2, COL_W, COL_W, PIER_H))
        # spread footing
        _append(verts, faces, _box(0.0, cy, foot + 0.4, COL_W + 1.2, COL_W + 1.2, 0.8))
    # X cross-brace between the two columns (two diagonal struts, Y-Z plane).
    z_hi = col_top - PIER_H * 0.15
    z_lo = col_top - PIER_H * 0.85
    _beam_yz(verts, faces, (0.0, -COL_HALF, z_lo), (0.0, COL_HALF, z_hi), 0.7)
    _beam_yz(verts, faces, (0.0, COL_HALF, z_lo), (0.0, -COL_HALF, z_hi), 0.7)
    # horizontal tie strut at mid-height
    _beam_yz(verts, faces, (0.0, -COL_HALF, (z_hi + z_lo) / 2),
             (0.0, COL_HALF, (z_hi + z_lo) / 2), 0.6)
    return mf.new_mesh_object("rail_viaduct_trestle", verts, faces, mat)


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
        out = "/tmp/viaduct_rail.glb"

    mf.reset_scene()
    m_deck = mf.palette_material("deck", "deck")
    m_ballast = mf.palette_material("structure_base", "structure_base")
    m_steel = mf.palette_material("structure", "structure")
    build_deck(m_deck)
    build_ballast(m_ballast)
    build_rails(m_deck)
    build_trestle(m_steel)

    if preview:
        import turntable
        turntable.render_turntable(preview)
    if out:
        mf.export_glb(out)


main()
