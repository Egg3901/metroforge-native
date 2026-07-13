"""gen_viaduct.py — generic tileable elevated ROAD viaduct segment (kit).

Run:  blender -b --factory-startup --python gen_viaduct.py -- <out.glb>
Add   --preview <prefix>  to render a turntable critique sheet.

ONE segment designed to TILE end-to-end along any elevated road run: a black
box-girder deck segment with a light fascia edge, riding on ONE integrated
white pier (column + flared pier cap / haunch) at the segment center. Placed
repeatedly by roads.rs at fixed spacing on gradeLevel>=1 runs (Medium+),
replacing the extruded slab+pier boxes; the cheap extrusion stays as the Potato
fallback.

Authoring frame (Blender Z-up, exported +Y up):
  * +X is the run direction; segment length SEG_LEN, meant to butt against the
    next instance (deck overlaps a hair to hide seams).
  * Deck TOP at z = 0 (matches the road ribbon deck plane). Deck hangs below.
  * Pier column runs down to z = -PIER_H (a representative one-grade-level
    drop); roads.rs vertical-scales the instance so the pier foot meets the
    sampled ground, so height is authored at scale ~1 for the common case.
  * Width along Y; Bevy scales Z to the real corridor width.

Poly budget ~600 tris/segment.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

SEG_LEN = 24.0
OVERLAP = 0.4        # deck overhang past the nominal length, hides tiling seams
DECK_W = 20.0
DECK_TH = 1.7        # box-girder depth
FASCIA_DROP = 0.9    # light edge fascia band hanging below the deck top edge
PIER_H = 9.0         # authored column drop (deck underside -> foot)
PIER_W = 3.2         # column footprint along X
PIER_D = 4.4         # column footprint along Y (across the road)
CAP_H = 1.3          # flared pier-cap / haunch under the deck
CAP_OVER = 2.2       # cap overhang past the column each side


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


def build_deck(mat):
    verts, faces = [], []
    length = SEG_LEN + OVERLAP
    # Box-girder deck: top at z=0, hanging DECK_TH below.
    _append(verts, faces, _box(0.0, 0.0, -DECK_TH / 2, length, DECK_W, DECK_TH))
    return mf.new_mesh_object("viaduct_deck", verts, faces, mat)


def build_fascia(mat):
    """Light fascia bands down both deck edges (the cel-outline read that lets
    the elevated deck separate from its shadow), plus a thin parapet lip."""
    verts, faces = [], []
    length = SEG_LEN + OVERLAP
    half = DECK_W / 2
    for s in (-1.0, 1.0):
        cy = s * (half - 0.35)
        # fascia band hanging just below the deck top edge
        _append(verts, faces, _box(0.0, cy, -FASCIA_DROP / 2 + 0.05,
                                   length, 0.7, FASCIA_DROP))
        # parapet barrier standing proud above the deck edge (safety wall)
        _append(verts, faces, _box(0.0, s * (half - 0.1), 0.55, length, 0.8, 1.1))
    return mf.new_mesh_object("viaduct_fascia", verts, faces, mat)


def build_pier(mat):
    verts, faces = [], []
    # Flared crosshead pier cap / haunch under the deck — extended along the
    # run (X) so it reads as a bearing seat that two deck segments land on.
    _append(verts, faces, _box(0.0, 0.0, -DECK_TH - CAP_H / 2,
                               PIER_W + 5.0, PIER_D + CAP_OVER, CAP_H))
    # Column down to the authored foot.
    col_top = -DECK_TH - CAP_H
    _append(verts, faces, _box(0.0, 0.0, col_top - PIER_H / 2,
                               PIER_W, PIER_D, PIER_H))
    # Small spread footing at the base.
    _append(verts, faces, _box(0.0, 0.0, col_top - PIER_H + 0.4,
                               PIER_W + 1.6, PIER_D + 1.6, 0.8))
    return mf.new_mesh_object("viaduct_pier", verts, faces, mat)


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
        out = "/tmp/viaduct_road.glb"

    mf.reset_scene()
    m_deck = mf.palette_material("deck", "deck")
    m_fascia = mf.palette_material("structure", "structure")
    m_pier = mf.palette_material("structure_side", "structure_side")
    build_deck(m_deck)
    build_fascia(m_fascia)
    build_pier(m_pier)

    if preview:
        import turntable
        turntable.render_turntable(preview)
    if out:
        mf.export_glb(out)


main()
