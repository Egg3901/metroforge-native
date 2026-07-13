"""gen_bridge.py — parametric suspension bridge family (Pilot A).

Run:  blender --background --python gen_bridge.py -- <variant> <out.glb>
      variant ∈ {generic, brooklyn}

Model faces +X along the span (deck runs along X), width along Z, up along
Blender +Z (exported to +Y). Origin at deck-center midspan, deck top at y=0
so Bevy can drop it in at the road-deck height and scale X to the real span.

Design (cel, low-poly, palette-material):
  * generic: two plain white portal towers, black deck ribbon, dark-grey main
    suspension cables sweeping in a catenary + vertical hangers.
  * brooklyn: TWIN STONE TOWERS with pointed DOUBLE gothic arches, DIAGONAL
    stay pattern fanning from tower tops to the deck — stylized, instantly
    'Brooklyn', not a photoreal replica.
The generic build is a base; brooklyn swaps the towers + adds diagonal stays.
"""

from __future__ import annotations

import sys
import math
import os

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402


# --- parametric knobs (meters) ------------------------------------------------
SPAN = 480.0        # tower-to-tower main span (Brooklyn main span ~486m)
DECK_W = 26.0       # deck width
DECK_T = 1.4        # deck thickness
TOWER_H = 84.0      # tower height above deck (Brooklyn ~84m)
OVERHANG = 90.0     # deck extension beyond each tower (approach)
HANGERS = 14        # vertical hangers per half-span


def _box(cx, cy, cz, sx, sy, sz):
    """Axis-aligned box centered at (cx,cy,cz) with half-not-full sizes sx,sy,sz."""
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


def _append_box(verts, faces, box):
    bv, bf = box
    off = len(verts)
    verts.extend(bv)
    faces.extend([tuple(i + off for i in face) for face in bf])


def build_deck(mat):
    total = SPAN + 2 * OVERHANG
    v, f = _box(0, 0, -DECK_T / 2, total, DECK_W, DECK_T)
    return mf.new_mesh_object("deck", v, f, mat)


def build_parapet(mat):
    total = SPAN + 2 * OVERHANG
    verts, faces = [], []
    for side in (-1, 1):
        _append_box(verts, faces,
                    _box(0, side * (DECK_W / 2 - 0.4), 0.6, total, 0.5, 1.2))
    return mf.new_mesh_object("parapet", verts, faces, mat)


def build_cables(mat, sag=0.5):
    """Main suspension cables (catenary approx) + vertical hangers, both sides."""
    verts, faces = [], []
    tower_x = SPAN / 2
    top_y = TOWER_H
    seg = 24
    for side in (-1, 1):
        z = side * (DECK_W / 2 - 1.2)
        # sampled catenary between the two tower tops
        pts = []
        for i in range(seg + 1):
            t = i / seg
            x = -tower_x + t * SPAN
            # parabolic sag, 0 at towers, deepest midspan
            y = top_y - sag * TOWER_H * (1 - (2 * t - 1) ** 2)
            pts.append((x, z, y))
        for i in range(seg):
            a, b = pts[i], pts[i + 1]
            _append_box(verts, faces, _cable_seg(a, b, 0.5))
        # vertical hangers
        for h in range(1, HANGERS):
            t = h / HANGERS
            x = -tower_x + t * SPAN
            cy = top_y - sag * TOWER_H * (1 - (2 * t - 1) ** 2)
            _append_box(verts, faces, _box(x, z, cy / 2, 0.35, 0.35, cy))
    return mf.new_mesh_object("cables", verts, faces, mat)


def _cable_seg(a, b, w):
    """A thin box connecting two 3D points (approx, axis of box along a->b)."""
    ax, az, ay = a  # note stored as (x, z, y)
    bx, bz, by = b
    mx, my, mz = (ax + bx) / 2, (ay + by) / 2, (az + bz) / 2
    dx, dy = bx - ax, by - ay
    length = math.hypot(dx, dy) + w
    v, f = _box(mx, mz, my, length, w, w)
    # rotate box in the X-up(Y) plane about Z to align with segment
    ang = math.atan2(dy, dx)
    ca, sa = math.cos(ang), math.sin(ang)
    rot = []
    for (x, zc, yv) in v:
        lx, ly = x - mx, yv - my
        rx = lx * ca - ly * sa
        ry = lx * sa + ly * ca
        rot.append((mx + rx, zc, my + ry))
    return rot, f


def build_tower_plain(cx, mat):
    """Generic portal tower: two legs + a cross beam at top and mid."""
    verts, faces = [], []
    for side in (-1, 1):
        z = side * (DECK_W / 2 - 1.0)
        _append_box(verts, faces, _box(cx, z, TOWER_H / 2, 4.0, 3.0, TOWER_H))
    _append_box(verts, faces, _box(cx, 0, TOWER_H - 3, 4.0, DECK_W - 2, 4.0))
    _append_box(verts, faces, _box(cx, 0, TOWER_H * 0.5, 4.0, DECK_W - 2, 3.0))
    return mf.new_mesh_object(f"tower_{cx:.0f}", verts, faces, mat)


def build_tower_brooklyn(cx, mat):
    """Stone tower with TWO pointed gothic arches. Built as a solid slab with
    two arch voids approximated by leaving leg gaps + a chevron gable — cel
    stylization, low-poly. Silhouette: wide base, two pointed openings, tapered
    cap."""
    verts, faces = [], []
    tw = DECK_W + 4          # tower is a broad wall across the deck
    base_h = 14.0            # solid base below arches
    arch_h = 34.0            # height of the arch openings
    pier = 3.5               # width of piers between/around arches
    # three piers (outer, center, outer) create two openings
    opening = (tw - 4 * pier) / 2
    # base slab
    _append_box(verts, faces, _box(cx, 0, base_h / 2, 6.0, tw, base_h))
    # piers rising through the arch zone
    pier_z = [-tw / 2 + pier / 2,
              -opening / 2 + 0 * pier,  # placeholder recomputed below
              tw / 2 - pier / 2]
    # compute pier centers cleanly: outer-left, center, outer-right
    left_out = -tw / 2 + pier / 2
    right_out = tw / 2 - pier / 2
    center = 0.0
    for pz in (left_out, center, right_out):
        _append_box(verts, faces,
                    _box(cx, pz, base_h + arch_h / 2, 6.0, pier, arch_h))
    # pointed arch chevrons (gables) over each opening — two triangular prisms
    for oz in (-(opening / 2 + pier / 2), (opening / 2 + pier / 2)):
        oz = oz  # opening center
    for oc in (-(pier + opening) / 2, (pier + opening) / 2):
        apex = base_h + arch_h + opening * 0.55
        # triangular prism: two tri faces + 3 quads (as a wedge)
        hw = opening / 2 + pier * 0.2
        y0 = base_h + arch_h
        d = 3.0
        off = len(verts)
        verts.extend([
            (cx - d, oc - hw, y0), (cx + d, oc - hw, y0),
            (cx + d, oc + hw, y0), (cx - d, oc + hw, y0),
            (cx - d, oc, apex),   (cx + d, oc, apex),
        ])
        faces.extend([
            (off + 0, off + 1, off + 2, off + 3),   # bottom
            (off + 0, off + 4, off + 5, off + 1),   # front slope
            (off + 3, off + 2, off + 5, off + 4),   # back slope
            (off + 0, off + 3, off + 4),            # left tri
            (off + 1, off + 5, off + 2),            # right tri
        ])
    # tapered cap above the whole tower
    cap_y = base_h + arch_h + opening * 0.55
    _append_box(verts, faces, _box(cx, 0, cap_y + 5, 6.0, tw * 0.7, 10.0))
    _append_box(verts, faces, _box(cx, 0, cap_y + 12, 6.0, tw * 0.4, 6.0))
    return mf.new_mesh_object(f"btower_{cx:.0f}", verts, faces, mat)


def build_diagonal_stays(mat):
    """Brooklyn's signature fan of diagonal stays from tower tops to deck."""
    verts, faces = [], []
    tower_x = SPAN / 2
    top_y = TOWER_H + 8
    n = 10
    for tx in (-tower_x, tower_x):
        for side in (-1, 1):
            z = side * (DECK_W / 2 - 1.5)
            for i in range(1, n + 1):
                # fan toward midspan and toward approach
                for direction in (-1, 1):
                    dx = direction * (i / n) * (SPAN / 2 - 10)
                    a = (tx, z, top_y)
                    b = (tx + dx, z, 0.2)
                    _append_box(verts, faces, _cable_seg(a, b, 0.28))
    return mf.new_mesh_object("stays", verts, faces, mat)


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    variant = argv[0] if argv else "generic"
    out = argv[1] if len(argv) > 1 else f"/tmp/bridge_{variant}.glb"

    mf.reset_scene()
    m_deck = mf.palette_material("deck", "deck")
    m_edge = mf.palette_material("deck_edge", "deck_edge")
    m_cable = mf.palette_material("cable", "cable")

    objs = [build_deck(m_deck), build_parapet(m_edge)]

    if variant == "brooklyn":
        m_stone = mf.palette_material("stone", "stone")
        objs.append(build_tower_brooklyn(-SPAN / 2, m_stone))
        objs.append(build_tower_brooklyn(SPAN / 2, m_stone))
        objs.append(build_cables(m_cable, sag=0.55))
        objs.append(build_diagonal_stays(m_cable))
    else:
        m_struct = mf.palette_material("structure", "structure")
        objs.append(build_tower_plain(-SPAN / 2, m_struct))
        objs.append(build_tower_plain(SPAN / 2, m_struct))
        objs.append(build_cables(m_cable, sag=0.5))

    mf.export_glb(out)


main()
