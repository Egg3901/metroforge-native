"""gen_rail_bridge.py — generic girder RAIL bridge for track water crossings.

Run:  blender -b --factory-startup --python gen_rail_bridge.py -- <out.glb>
Add   --preview <prefix>  to render a turntable critique sheet.

Track water crossings currently render nothing (transit.rs only draws side
rails on the flat ribbon). This is a simple, cheap DECK-GIRDER rail bridge: two
deep white plate girders flanking a black rail deck, regularly spaced vertical
stiffener ribs (the plate-girder read), lateral X cross-bracing slung under the
deck between the girders, a grey ballast bed + two rails on top, and a squat
white abutment pier at each end. A single-span model scaled X -> real crossing
(like bridges.rs); height (Y) is left unstretched.

Authoring frame (Blender Z-up, exported +Y up):
  * +X span axis; deck TOP at z = 0; width along Y. Authored at a
    representative 120m span; Bevy scales X to the real over-water chord.

Poly budget ~1200 tris.
"""

from __future__ import annotations

import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

SPAN = 120.0
OVERHANG = 8.0        # deck runs onto the abutments at each end
DECK_W = 9.0
DECK_TH = 1.4
GIRDER_H = 10.0       # plate-girder depth below the deck
GIRDER_T = 0.7        # girder plate thickness
STIFFENERS = 12       # vertical stiffener ribs per girder
BALLAST_W = 4.4
BALLAST_H = 0.55
ABUT_H = 11.0         # abutment pier height below deck
ABUT_W = 6.0          # abutment length along X


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


def _beam_xy(verts, faces, a, b, w, z):
    """Flat square beam between a,b in the X-Y plane at height z (lateral brace)."""
    ax, ay = a
    bx, by = b
    mx, my = (ax + bx) / 2, (ay + by) / 2
    d1, d2 = bx - ax, by - ay
    length = math.hypot(d1, d2) + w
    v, f = _box(mx, my, z, length, w, w)
    ang = math.atan2(d2, d1); ca, sa = math.cos(ang), math.sin(ang)
    rot = [(mx + (x - mx) * ca - (y - my) * sa, my + (x - mx) * sa + (y - my) * ca, zz)
           for (x, y, zz) in v]
    off = len(verts)
    verts.extend(rot)
    faces.extend([tuple(i + off for i in fc) for fc in f])


def build_deck(mat):
    verts, faces = [], []
    length = SPAN + OVERHANG * 2
    _append(verts, faces, _box(0.0, 0.0, -DECK_TH / 2, length, DECK_W, DECK_TH))
    return mf.new_mesh_object("rail_bridge_deck", verts, faces, mat)


def build_ballast(mat_ballast, mat_rail):
    verts, faces = [], []
    length = SPAN + OVERHANG * 2
    _append(verts, faces, _box(0.0, 0.0, BALLAST_H / 2, length, BALLAST_W, BALLAST_H))
    ob = mf.new_mesh_object("rail_bridge_ballast", verts, faces, mat_ballast)
    rv, rf = [], []
    for s in (-1.0, 1.0):
        _append(rv, rf, _box(0.0, s * 0.9, BALLAST_H, length, 0.26, 0.36))
    mf.new_mesh_object("rail_bridge_rails", rv, rf, mat_rail)
    return ob


def build_girders(mat_web, mat_detail):
    """Web in structure white; stiffener ribs + bottom flange in the shade tone
    so the plate-girder texture (vertical ribs, a bottom chord line) reads."""
    half_w = DECK_W / 2
    zc = -DECK_TH - GIRDER_H / 2
    web_v, web_f = [], []
    det_v, det_f = [], []
    for s in (-1.0, 1.0):
        cy = s * (half_w - GIRDER_T / 2)
        _append(web_v, web_f, _box(0.0, cy, zc, SPAN, GIRDER_T, GIRDER_H))
        # bottom flange (slightly wider) — shade tone chord line
        _append(det_v, det_f, _box(0.0, cy, -DECK_TH - GIRDER_H + 0.25,
                                   SPAN, GIRDER_T + 0.8, 0.5))
        # vertical stiffener ribs proud of the web — shade tone
        for i in range(STIFFENERS + 1):
            x = -SPAN / 2 + SPAN * i / STIFFENERS
            _append(det_v, det_f, _box(x, cy + s * 0.28, zc, 0.5, 0.4, GIRDER_H))
    mf.new_mesh_object("rail_bridge_girder_detail", det_v, det_f, mat_detail)
    return mf.new_mesh_object("rail_bridge_girders", web_v, web_f, mat_web)


def build_bracing(mat):
    """Lateral X cross-bracing slung under the deck between the two girders."""
    verts, faces = [], []
    half_w = DECK_W / 2 - GIRDER_T
    z = -DECK_TH - GIRDER_H + 0.7
    panels = 6
    for i in range(panels):
        xa = -SPAN / 2 + SPAN * i / panels
        xb = -SPAN / 2 + SPAN * (i + 1) / panels
        _beam_xy(verts, faces, (xa, -half_w), (xb, half_w), 0.5, z)
        _beam_xy(verts, faces, (xa, half_w), (xb, -half_w), 0.5, z)
    return mf.new_mesh_object("rail_bridge_bracing", verts, faces, mat)


def build_abutments(mat):
    verts, faces = [], []
    for s in (-1.0, 1.0):
        cx = s * (SPAN / 2 + OVERHANG - ABUT_W / 2)
        _append(verts, faces, _box(cx, 0.0, -DECK_TH - ABUT_H / 2,
                                   ABUT_W, DECK_W + 1.5, ABUT_H))
    return mf.new_mesh_object("rail_bridge_abutments", verts, faces, mat)


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
        out = "/tmp/rail_bridge.glb"

    mf.reset_scene()
    m_deck = mf.palette_material("deck", "deck")
    m_ballast = mf.palette_material("structure_base", "structure_base")
    m_steel = mf.palette_material("structure", "structure")
    m_shade = mf.palette_material("structure_side", "structure_side")
    build_deck(m_deck)
    build_ballast(m_ballast, m_deck)
    build_girders(m_steel, m_shade)
    build_bracing(m_shade)
    build_abutments(m_shade)

    if preview:
        import turntable
        turntable.render_turntable(preview)
    if out:
        mf.export_glb(out)


main()
