"""gen_bridge.py — suspension bridge family (rebuilt under the model-craft loop).

Run:  blender -b --factory-startup --python gen_bridge.py -- <variant> <out.glb>
      variant in {generic, brooklyn}
Add   --preview <prefix>  to also render a turntable critique sheet.

Model faces +X along the span, width along Y, up along Z. Origin at deck-center
midspan, deck TOP at z=0 so Bevy drops it at the road-deck height and scales X
to the real span. glTF export flips Z-up -> Bevy +Y up.

BROOKLYN (signature): massive warm-stone twin towers, each with TWO pointed
gothic arch openings the deck passes through; deck ~halfway up the towers; FOUR
main cables in catenary from saddles over the tower tops to a low point near
mid-deck; a dense vertical hanger curtain PLUS the signature diagonal stay fan
radiating from the tower tops (the hybrid web is THE recognizable feature);
heavy dark truss deck band with a slight camber; big stone anchorage blocks at
both ends.

GENERIC: simpler steel portal towers (no stone, no arches), TWO cables, hanger
curtain, NO stay fan — must NOT read as Brooklyn at distance.
"""

from __future__ import annotations

import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

# --- proportions (model meters; game compresses under-deck clearance) --------
SPAN = 486.0        # tower-to-tower main span (Brooklyn ~486m)
OVERHANG = 90.0     # side span deck beyond each tower toward the anchorages
DECK_W = 26.0       # deck width
DECK_BAND = 4.2     # truss-band depth (deck reads HEAVY, not a slab)
CAMBER = 1.2        # deck rises this much at midspan (kept below the roads.rs
                    # ribbon so the two black decks never z-fight in-game)

# Real Brooklyn towers are 84m above WATER with the deck ~40m below the top.
# The game compresses under-deck clearance: the deck sits only ~8m above the
# water (roads.rs BRIDGE_DECK_Y), so authoring the real 44m deck-to-top put
# most of the 84m of masonry UNDERWATER in-game and the tower read ~25-30m
# tall (owner-rejected in-game shot, 2026-07-13). Tower height is therefore
# authored ABOVE THE DECK so the in-game silhouette carries the real
# 84m-above-water proportion.
TOWER_TX = 12.0     # tower thickness along the span (chunky masonry)
TOWER_W = DECK_W + 9.0   # tower width across the deck
TOWER_TOP = 76.0    # tower top above deck (deck at ~8m -> ~84m above water)
TOWER_BASE = -12.0  # masonry foot below deck (into the compressed water gap)

# pointed-arch cutout geometry (per tower) — tall gothic openings
ARCH_BOTTOM = -4.0      # opening springs from just below the deck band
ARCH_SPRING = 46.0      # top of the rectangular part of the opening
ARCH_APEX = 64.0        # point of the gothic arch
PIER_W = 4.5            # masonry pier width (3 piers -> 2 openings)

HANGERS = 22            # vertical hangers per span (dense curtain)
CABLE_SAG = 0.86       # fraction of tower height the cable low point drops to

# Member sections (meters). Deliberately fatter than scale-true steel: at the
# game camera (600-800m oblique) a 0.85m cable was sub-pixel and the whole web
# vanished in-game; these read as "a dark suggestion" at range while staying
# thin against the 84m towers up close.
CABLE_W = 1.5           # main catenary + backstays
HANGER_W = 0.55         # vertical hanger curtain
STAY_W = 0.5            # diagonal stay fan


# ----------------------------------------------------------------------------
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


def _beam(verts, faces, a, b, w):
    """A square-section beam of side `w` between world points a,b (x,y,z).
    Rotated only in the X-Z (span/height) plane — good for cables, stays, arch
    soffits which all live in a vertical plane at a fixed Y."""
    ax, ay, az = a
    bx, by, bz = b
    mx, my, mz = (ax + bx) / 2, (ay + by) / 2, (az + bz) / 2
    dx, dz = bx - ax, bz - az
    length = math.hypot(dx, dz) + w
    v, f = _box(mx, my, mz, length, w, w)
    ang = math.atan2(dz, dx)
    ca, sa = math.cos(ang), math.sin(ang)
    rot = []
    for (x, y, z) in v:
        lx, lz = x - mx, z - mz
        rot.append((mx + lx * ca - lz * sa, y, mz + lx * sa + lz * ca))
    off = len(verts)
    verts.extend(rot)
    faces.extend([tuple(i + off for i in fc) for fc in f])


def _beam_y(verts, faces, a, b, thick_x, w):
    """Beam between two points that differ in Y and Z (fixed X) — for the arch
    soffits which slant across the deck width. `thick_x` = span-direction
    thickness so the soffit reads as full masonry depth."""
    ax, ay, az = a
    bx, by, bz = b
    mx, my, mz = (ax + bx) / 2, (ay + by) / 2, (az + bz) / 2
    dy, dz = by - ay, bz - az
    length = math.hypot(dy, dz) + w
    v, f = _box(mx, my, mz, thick_x, length, w)
    ang = math.atan2(dz, dy)
    ca, sa = math.cos(ang), math.sin(ang)
    rot = []
    for (x, y, z) in v:
        ly, lz = y - my, z - mz
        rot.append((x, my + ly * ca - lz * sa, mz + ly * sa + lz * ca))
    off = len(verts)
    verts.extend(rot)
    faces.extend([tuple(i + off for i in fc) for fc in f])


def _camber_z(x):
    """Deck-top height at span position x (parabolic camber, 0 at the ends)."""
    half = SPAN / 2 + OVERHANG
    t = max(-1.0, min(1.0, x / half))
    return CAMBER * (1 - t * t)


# ----------------------------------------------------------------------------
def build_deck(mat_deck):
    """Heavy truss deck band with a slight camber, built as segments."""
    total = SPAN + 2 * OVERHANG
    n = 28
    verts, faces = [], []
    x0 = -total / 2
    step = total / n
    for i in range(n):
        cx = x0 + step * (i + 0.5)
        top = _camber_z(cx)
        _append(verts, faces, _box(cx, 0, top - DECK_BAND / 2, step * 1.02, DECK_W, DECK_BAND))
    return mf.new_mesh_object("deck", verts, faces, mat_deck)


def build_deck_truss(mat):
    """Diagonal truss webbing on both deck edges -> reads as a truss, not slab."""
    total = SPAN + 2 * OVERHANG
    n = 40
    verts, faces = [], []
    x0 = -total / 2
    step = total / n
    for side in (-1, 1):
        y = side * (DECK_W / 2 - 0.3)
        for i in range(n):
            xa = x0 + step * i
            xb = x0 + step * (i + 1)
            za = _camber_z(xa)
            zb = _camber_z(xb)
            if i % 2 == 0:
                _beam(verts, faces, (xa, y, za), (xb, y, zb - DECK_BAND), 0.35)
            else:
                _beam(verts, faces, (xa, y, za - DECK_BAND), (xb, y, zb), 0.35)
        _beam(verts, faces, (x0, y, _camber_z(x0)), (x0 + total, y, _camber_z(x0)), 0.4)
    return mf.new_mesh_object("deck_truss", verts, faces, mat)


def build_tower_brooklyn(cx, mat):
    """Massive stone tower with TWO pointed gothic arch openings the deck
    passes through. Positive-space masonry (no booleans): base + 3 piers +
    upper spandrel band + pointed arch soffits + cornice cap; the two pointed
    voids are what is left between them."""
    verts, faces = [], []
    tw = TOWER_W
    tx = TOWER_TX
    pier_c = [(-tw / 2 + PIER_W / 2), 0.0, (tw / 2 - PIER_W / 2)]
    opening_w = (tw - 3 * PIER_W) / 2
    open_c = [-(PIER_W + opening_w) / 2, (PIER_W + opening_w) / 2]

    # solid base below the arch openings
    _append(verts, faces, _box(cx, 0, (TOWER_BASE + ARCH_BOTTOM) / 2, tx, tw,
                               ARCH_BOTTOM - TOWER_BASE))
    # three piers rising the FULL arch height (base -> spandrel). The two
    # openings stay distinct because the center pier is solid all the way up.
    for pc in pier_c:
        _append(verts, faces, _box(cx, pc, (ARCH_BOTTOM + ARCH_APEX) / 2,
                                   tx, PIER_W, ARCH_APEX - ARCH_BOTTOM))
    # pointed arch soffits: two slanted masonry wedges per opening close the
    # top of each rectangular void into a gothic point at the apex.
    for oc in open_c:
        left = oc - opening_w / 2
        right = oc + opening_w / 2
        _beam_y(verts, faces, (cx, left, ARCH_SPRING), (cx, oc, ARCH_APEX), tx, 1.8)
        _beam_y(verts, faces, (cx, right, ARCH_SPRING), (cx, oc, ARCH_APEX), tx, 1.8)
    # solid spandrel band above the arches (full width) up to the tower top
    _append(verts, faces, _box(cx, 0, (ARCH_APEX + TOWER_TOP) / 2, tx, tw,
                               TOWER_TOP - ARCH_APEX))
    # cornice cap (slight overhang) + saddle block the cables ride over
    _append(verts, faces, _box(cx, 0, TOWER_TOP + 1.6, tx + 2.5, tw + 2.5, 3.2))
    _append(verts, faces, _box(cx, 0, TOWER_TOP + 4.0, tx * 0.7, tw, 2.0))
    return mf.new_mesh_object(f"btower_{cx:.0f}", verts, faces, mat)


def build_tower_portal(cx, mat):
    """Generic STEEL portal tower: two slim legs + two cross struts. No stone,
    no arches — deliberately not Brooklyn."""
    verts, faces = [], []
    leg = 5.0                    # chunky enough to read white-on-grey at range
    # legs only rise from just below the deck (steel tower, no deep masonry foot)
    base = -6.0
    for side in (-1, 1):
        y = side * (DECK_W / 2 - 1.0)
        _append(verts, faces, _box(cx, y, (base + TOWER_TOP) / 2, leg, leg,
                                   TOWER_TOP - base))
    # two portal cross-struts (top + upper-mid) — the steel-portal signature
    _append(verts, faces, _box(cx, 0, TOWER_TOP - 3, leg, DECK_W, 4.0))
    _append(verts, faces, _box(cx, 0, TOWER_TOP * 0.55, leg, DECK_W, 3.2))
    return mf.new_mesh_object(f"ptower_{cx:.0f}", verts, faces, mat)


def _cable_y_positions(n):
    if n == 2:
        return [-(DECK_W / 2 - 2.0), (DECK_W / 2 - 2.0)]
    inner = DECK_W / 2 - 8.0
    outer = DECK_W / 2 - 2.0
    return [-outer, -inner, inner, outer]


def build_cables(mat, n_cables, saddle_z):
    """Main catenary cables from tower saddle to tower saddle, low point near
    mid-deck; plus a dense vertical hanger curtain hung off each cable."""
    verts, faces = [], []
    tower_x = SPAN / 2
    low_z = saddle_z - CABLE_SAG * (saddle_z - DECK_BAND)
    seg = 26

    def cable_z(t):
        return low_z + (saddle_z - low_z) * (2 * t - 1) ** 2

    ys = _cable_y_positions(n_cables)
    for y in ys:
        pts = []
        for i in range(seg + 1):
            t = i / seg
            x = -tower_x + t * SPAN
            pts.append((x, y, cable_z(t)))
        for i in range(seg):
            _beam(verts, faces, pts[i], pts[i + 1], CABLE_W)
        anchor_x = tower_x + OVERHANG
        for sgn in (-1, 1):
            _beam(verts, faces, (sgn * tower_x, y, saddle_z),
                  (sgn * anchor_x, y, DECK_BAND + 2), CABLE_W)
    for y in (ys[0], ys[-1]):
        for h in range(1, HANGERS):
            t = h / HANGERS
            x = -tower_x + t * SPAN
            top = cable_z(t)
            bot = _camber_z(x)
            _append(verts, faces, _box(x, y, (top + bot) / 2, HANGER_W, HANGER_W, top - bot))
    return mf.new_mesh_object("cables", verts, faces, mat)


def build_stay_fan(mat, saddle_z):
    """Brooklyn's signature diagonal stays radiating from the tower tops down
    to the deck (overlapping the hangers to make the hybrid web)."""
    verts, faces = [], []
    tower_x = SPAN / 2
    n = 8
    mid_reach = SPAN / 2 - 16      # stays land within the main span
    anchor_reach = OVERHANG - 8    # and within the side span (never past deck)
    for tpos in (-tower_x, tower_x):
        s = 1.0 if tpos > 0 else -1.0
        for y in (-(DECK_W / 2 - 1.5), (DECK_W / 2 - 1.5)):
            for i in range(1, n + 1):
                t = i / n
                for bx in (tpos - s * t * mid_reach, tpos + s * t * anchor_reach):
                    _beam(verts, faces, (tpos, y, saddle_z),
                          (bx, y, _camber_z(bx) + 0.4), STAY_W)
    return mf.new_mesh_object("stays", verts, faces, mat)


def build_anchorages(mat):
    """Massive stone anchorage blocks beyond each side span."""
    verts, faces = [], []
    ax = SPAN / 2 + OVERHANG - 6
    for sgn in (-1, 1):
        # big block grounded below the deck, rising just above it (the cable
        # backstays die into its top) — reads as a massive masonry anchorage.
        _append(verts, faces, _box(sgn * ax, 0, -4.0, 40.0, DECK_W + 12, 44.0))
        _append(verts, faces, _box(sgn * ax, 0, 20.0, 30.0, DECK_W + 6, 8.0))
    return mf.new_mesh_object("anchorages", verts, faces, mat)


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    variant = argv[0] if argv else "generic"
    out = None
    preview = None
    rest = argv[1:]
    i = 0
    while i < len(rest):
        if rest[i] == "--preview":
            preview = rest[i + 1]
            i += 2
        else:
            out = rest[i]
            i += 1
    if out is None:
        out = f"/tmp/bridge_{variant}.glb"

    mf.reset_scene()
    m_deck = mf.palette_material("deck", "deck")
    m_edge = mf.palette_material("deck_edge", "deck_edge")
    m_cable = mf.palette_material("cable", "cable")

    build_deck(m_deck)
    build_deck_truss(m_edge)

    if variant == "brooklyn":
        m_stone = mf.palette_material("stone", "stone")
        saddle = TOWER_TOP + 4.0
        build_tower_brooklyn(-SPAN / 2, m_stone)
        build_tower_brooklyn(SPAN / 2, m_stone)
        build_anchorages(m_stone)
        build_cables(m_cable, 4, saddle)
        build_stay_fan(m_cable, saddle)
    else:
        m_struct = mf.palette_material("structure", "structure")
        saddle = TOWER_TOP + 1.0
        build_tower_portal(-SPAN / 2, m_struct)
        build_tower_portal(SPAN / 2, m_struct)
        build_cables(m_cable, 2, saddle)

    if preview:
        import turntable
        turntable.render_turntable(preview)
    if out:
        mf.export_glb(out)


main()
