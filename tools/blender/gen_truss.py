"""gen_truss.py — generic Warren through-truss bridge (model-craft loop).

Run:  blender -b --factory-startup --python gen_truss.py -- <out.glb>
Add   --preview <prefix>  to render a turntable critique sheet.

A new family so mid-length crossings (120-250m) stop using flat deck ribbons.
Through-truss BOX: two Warren truss side walls (top + bottom chords with a
zig-zag diagonal web and vertical posts), tied together by top lateral bracing
(the box lid) and inward-leaning portal frames at each end. The roadway passes
THROUGH the box at the bottom chord level. White steel structure, black deck.

Model faces +X along the span, width along Y, deck top at z=0. Authored at a
representative 180m span; Bevy scales X to the real crossing.
"""

from __future__ import annotations

import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

SPAN = 180.0
DECK_W = 16.0
DECK_BAND = 1.8
TRUSS_H = 22.0
PANELS = 10
CHORD = 1.1     # chord/post/diagonal square section
DIAG = 0.9


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


def _beam(verts, faces, a, b, w, plane):
    """Square beam between a,b. plane='xz' rotates in span-height, 'yz' in
    width-height, 'xy' stays flat (top lateral)."""
    ax, ay, az = a
    bx, by, bz = b
    mx, my, mz = (ax + bx) / 2, (ay + by) / 2, (az + bz) / 2
    if plane == "xz":
        d1, d2 = bx - ax, bz - az
        length = math.hypot(d1, d2) + w
        v, f = _box(mx, my, mz, length, w, w)
        ang = math.atan2(d2, d1); ca, sa = math.cos(ang), math.sin(ang)
        rot = [(mx + (x - mx) * ca - (z - mz) * sa, y, mz + (x - mx) * sa + (z - mz) * ca)
               for (x, y, z) in v]
    elif plane == "yz":
        d1, d2 = by - ay, bz - az
        length = math.hypot(d1, d2) + w
        v, f = _box(mx, my, mz, w, length, w)
        ang = math.atan2(d2, d1); ca, sa = math.cos(ang), math.sin(ang)
        rot = [(x, my + (y - my) * ca - (z - mz) * sa, mz + (y - my) * sa + (z - mz) * ca)
               for (x, y, z) in v]
    else:  # xy flat
        d1, d2 = bx - ax, by - ay
        length = math.hypot(d1, d2) + w
        v, f = _box(mx, my, mz, length, w, w)
        ang = math.atan2(d2, d1); ca, sa = math.cos(ang), math.sin(ang)
        rot = [(mx + (x - mx) * ca - (y - my) * sa, my + (x - mx) * sa + (y - my) * ca, z)
               for (x, y, z) in v]
    off = len(verts)
    verts.extend(rot)
    faces.extend([tuple(i + off for i in fc) for fc in f])


def build_truss(mat):
    verts, faces = [], []
    total = SPAN
    x0 = -total / 2
    step = total / PANELS
    for y in (-DECK_W / 2, DECK_W / 2):
        # top + bottom chords
        _beam(verts, faces, (x0, y, 0.0), (x0 + total, y, 0.0), CHORD, "xz")
        _beam(verts, faces, (x0, y, TRUSS_H), (x0 + total, y, TRUSS_H), CHORD, "xz")
        # vertical posts at every node
        for i in range(PANELS + 1):
            x = x0 + step * i
            _beam(verts, faces, (x, y, 0.0), (x, y, TRUSS_H), CHORD * 0.85, "xz")
        # Warren zig-zag diagonals (alternating up/down each panel)
        for i in range(PANELS):
            xa = x0 + step * i
            xb = x0 + step * (i + 1)
            if i % 2 == 0:
                _beam(verts, faces, (xa, y, 0.0), (xb, y, TRUSS_H), DIAG, "xz")
            else:
                _beam(verts, faces, (xa, y, TRUSS_H), (xb, y, 0.0), DIAG, "xz")
    # top lateral bracing (the box lid): cross beams + X-braces per panel
    for i in range(PANELS + 1):
        x = x0 + step * i
        _beam(verts, faces, (x, -DECK_W / 2, TRUSS_H), (x, DECK_W / 2, TRUSS_H), CHORD * 0.8, "yz")
    for i in range(PANELS):
        xa = x0 + step * i
        xb = x0 + step * (i + 1)
        _beam(verts, faces, (xa, -DECK_W / 2, TRUSS_H), (xb, DECK_W / 2, TRUSS_H), DIAG * 0.7, "xy")
        _beam(verts, faces, (xa, DECK_W / 2, TRUSS_H), (xb, -DECK_W / 2, TRUSS_H), DIAG * 0.7, "xy")
    # inward-leaning portal frames at each end (the through-truss entrance)
    for xe in (x0, x0 + total):
        for y in (-DECK_W / 2, DECK_W / 2):
            _beam(verts, faces, (xe, y, TRUSS_H * 0.55),
                  (xe, y * 0.55, TRUSS_H), CHORD, "yz")
    return mf.new_mesh_object("truss", verts, faces, mat)


def build_deck(mat):
    verts, faces = [], []
    _append(verts, faces, _box(0, 0, -DECK_BAND / 2, SPAN + 8, DECK_W - 1.0, DECK_BAND))
    return mf.new_mesh_object("deck", verts, faces, mat)


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
        out = "/tmp/bridge_truss.glb"

    mf.reset_scene()
    m_struct = mf.palette_material("structure", "structure")
    m_deck = mf.palette_material("deck", "deck")
    build_deck(m_deck)
    build_truss(m_struct)

    if preview:
        import turntable
        turntable.render_turntable(preview)
    if out:
        mf.export_glb(out)


main()
