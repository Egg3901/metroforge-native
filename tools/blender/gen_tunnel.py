"""gen_tunnel.py — generic concrete tunnel entrance / portal (model-craft loop).

Run:  blender -b --factory-startup --python gen_tunnel.py -- <out.glb>
Add   --preview <prefix>  to render a turntable critique sheet.

A single reusable portal that replaces the box/trapezoid portal marks for BOTH
road grade-separation tunnels (roads.rs) AND metro track tunnels (transit.rs).
A concrete portal FACE FRAME (two abutment posts + a lintel + a coping course)
around a RECESSED near-black mouth, flanked by short flaring wing/approach walls
that read as the road cut diving underground.

Authoring frame (Blender Z-up, exported +Y up):
  * Origin at the mouth center on the ground; ground plane z = 0.
  * +X is the OUTWARD / approach direction (the face you see, the road comes
    from +X and dives to -X into the hillside). The dark mouth recesses to -X.
  * Width along Y. Authored at a canonical mouth so Bevy scales Z to the real
    corridor width; height (Y after export) is left at scale 1.

Poly budget ~800 tris. White concrete structure, near-black recessed mouth.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

# Canonical mouth opening (Bevy scales width Y -> real corridor; height stays 1).
MOUTH_W = 12.0
MOUTH_H = 8.0
FRAME = 2.2          # concrete border thickness around the opening
FACE_T = 1.6         # portal face-wall thickness along X
MOUTH_DEPTH = 7.0    # how far the dark recess reads back into -X
WING_LEN = 7.5       # flaring approach wall length along +X
WING_DROP = 3.6      # wing wall top slopes down toward the approach
COPING = 0.9         # coping course extra height + overhang


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


def _quad(a, b, c, d):
    return [a, b, c, d], [(0, 1, 2, 3)]


def _append(verts, faces, geo):
    bv, bf = geo
    off = len(verts)
    verts.extend(bv)
    faces.extend([tuple(i + off for i in fc) for fc in bf])


def build_frame(mat):
    verts, faces = [], []
    half_w = MOUTH_W / 2
    post_cx = -FACE_T / 2  # face wall centered so its front face sits at x≈0
    # Two abutment posts flanking the opening.
    for s in (-1.0, 1.0):
        cy = s * (half_w + FRAME / 2)
        _append(verts, faces, _box(post_cx, cy, MOUTH_H / 2 + 0.0,
                                   FACE_T, FRAME, MOUTH_H + FRAME))
    # Lintel bar across the top of the opening.
    _append(verts, faces, _box(post_cx, 0.0, MOUTH_H + FRAME / 2,
                               FACE_T, MOUTH_W + FRAME * 2, FRAME))
    return mf.new_mesh_object("portal_frame", verts, faces, mat)


def build_coping(mat):
    """Overhanging cap band along the top of the face — a slightly darker
    grounding shade so the horizontal lintel/coping reads as a distinct
    element at game distance (not one flat white mass)."""
    verts, faces = [], []
    post_cx = -FACE_T / 2
    _append(verts, faces, _box(post_cx - 0.15, 0.0, MOUTH_H + FRAME + COPING / 2,
                               FACE_T + 0.9, MOUTH_W + FRAME * 2 + 1.2, COPING))
    return mf.new_mesh_object("portal_coping", verts, faces, mat)


def build_wings(mat):
    """Two flaring retaining/approach walls sweeping out along +X, tops sloping
    down toward the approach so the cut reads as diving underground."""
    verts, faces = [], []
    half_w = MOUTH_W / 2
    base_y = half_w + FRAME
    flare = 1.4  # how far the far end splays outward in Y
    for s in (-1.0, 1.0):
        y_in = s * base_y
        y_out = s * (base_y + flare)
        x0 = 0.0
        x1 = WING_LEN
        h_in = MOUTH_H + FRAME
        h_out = MOUTH_H + FRAME - WING_DROP - 2.0
        th = 1.8  # wall thickness in Y (outward face)
        yo = s * th
        # Inner face bottom rectangle, sloped-top wall as a 6-face solid.
        v0 = (x0, y_in, 0.0)
        v1 = (x1, y_out, 0.0)
        v2 = (x1, y_out + yo, 0.0)
        v3 = (x0, y_in + yo, 0.0)
        v4 = (x0, y_in, h_in)
        v5 = (x1, y_out, h_out)
        v6 = (x1, y_out + yo, h_out)
        v7 = (x0, y_in + yo, h_in)
        off = len(verts)
        verts.extend([v0, v1, v2, v3, v4, v5, v6, v7])
        for fc in [(0, 1, 2, 3), (4, 7, 6, 5), (0, 4, 5, 1),
                   (1, 5, 6, 2), (2, 6, 7, 3), (3, 7, 4, 0)]:
            faces.append(tuple(i + off for i in fc))
    return mf.new_mesh_object("portal_wings", verts, faces, mat)


def build_mouth(mat):
    """The recessed near-black mouth: interior box set back into -X plus a floor
    apron, so the opening reads as a real hole, not paint."""
    verts, faces = [], []
    half_w = MOUTH_W / 2
    # Interior tube (open toward +X): back wall, top, two sides, floor.
    x0 = -0.2
    x1 = -MOUTH_DEPTH
    # back wall
    _append(verts, faces, _quad(
        (x1, -half_w, 0.0), (x1, half_w, 0.0),
        (x1, half_w, MOUTH_H), (x1, -half_w, MOUTH_H)))
    # top
    _append(verts, faces, _quad(
        (x0, -half_w, MOUTH_H), (x0, half_w, MOUTH_H),
        (x1, half_w, MOUTH_H), (x1, -half_w, MOUTH_H)))
    # sides
    for s in (-1.0, 1.0):
        y = s * half_w
        _append(verts, faces, _quad(
            (x0, y, 0.0), (x0, y, MOUTH_H),
            (x1, y, MOUTH_H), (x1, y, 0.0)))
    return mf.new_mesh_object("portal_mouth", verts, faces, mat)


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
        out = "/tmp/portal_tunnel.glb"

    mf.reset_scene()
    m_struct = mf.palette_material("structure", "structure")
    m_shade = mf.palette_material("structure_side", "structure_side")
    m_base = mf.palette_material("structure_base", "structure_base")
    m_mouth = mf.palette_material("mouth", "deck")
    build_frame(m_struct)
    build_coping(m_base)
    build_wings(m_shade)
    build_mouth(m_mouth)

    if preview:
        import turntable
        turntable.render_turntable(preview)
    if out:
        mf.export_glb(out)


main()
