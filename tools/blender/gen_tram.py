"""gen_tram.py — 2-section articulated low-floor tram, model-craft loop.

Run:  blender -b --factory-startup --python gen_tram.py -- <out.glb>
Add   --preview <prefix>  to render a turntable critique sheet.

Silhouette checklist (score from the turntable BEFORE trusting the .glb):
  * SIDE : low-floor silhouette (body sits low, small wheels), BIG windows
           (a tall continuous band), a visible articulation gap/bellows hint
           between the two sections, rounded cab hint at each end.
  * FRONT: a rounded (chamfered-top) cab with a dark wrap screen.
  * total length ~1.6x the bus (~19m); <2000 tris.

Orientation: tram runs along +X, base at z=0. BODY near-white ('transit_body')
for per-route tint; windows / bellows neutral dark, roof neutral grey.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

SECTIONS = 2
SEC_LEN = 9.0
BELLOWS = 1.0        # articulation gap between sections
WIDTH = 2.65
HEIGHT = 3.3
FLOOR = 0.35         # LOW floor: body rides low, small wheels below
ROOF_RISE = 0.30
CAB_CHAMFER = 1.2    # front/rear roof taper length -> rounded cab read
TOTAL = SECTIONS * SEC_LEN + (SECTIONS - 1) * BELLOWS  # ~19m ~= 1.6x bus


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


def _section_centers():
    x0 = -TOTAL / 2
    return [x0 + SEC_LEN / 2 + s * (SEC_LEN + BELLOWS) for s in range(SECTIONS)]


def build_bodies(mat):
    """Two low-floor section boxes; the end sections' outer tops are chamfered
    (a short lower slab at each nose) to read as a rounded cab silhouette."""
    verts, faces = [], []
    top = HEIGHT - ROOF_RISE
    body_h = top - FLOOR
    centers = _section_centers()
    for cx in centers:
        _append(verts, faces, _box(cx, 0, (FLOOR + top) / 2, SEC_LEN * 0.98, WIDTH, body_h))
    # cab noses: a lowered wedge-ish cap at each outer end (single lower box)
    ends = [(centers[0], -1), (centers[-1], 1)]
    for cx, sgn in ends:
        nx = cx + sgn * (SEC_LEN / 2 - CAB_CHAMFER / 2) * 0.98
        _append(verts, faces,
                _box(nx, 0, FLOOR + body_h * 0.42, CAB_CHAMFER, WIDTH * 0.98, body_h * 0.84))
    return mf.new_mesh_object("bodies", verts, faces, mat)


def build_roof(mat):
    verts, faces = [], []
    top = HEIGHT - ROOF_RISE
    for cx in _section_centers():
        _append(verts, faces,
                _box(cx, 0, top + ROOF_RISE / 2, SEC_LEN * 0.9, WIDTH * 0.82, ROOF_RISE))
    return mf.new_mesh_object("roof", verts, faces, mat)


def build_glazing(mat):
    """BIG windows: a tall continuous band on both sides across both sections,
    plus a wrap cab screen on each outer nose."""
    verts, faces = [], []
    centers = _section_centers()
    band_z = WIDTH / 2 + 0.02
    band_cy = FLOOR + (HEIGHT - ROOF_RISE - FLOOR) * 0.58
    band_h = (HEIGHT - ROOF_RISE - FLOOR) * 0.62  # big windows
    for cx in centers:
        for side in (-1, 1):
            _append(verts, faces, _box(cx, side * band_z, band_cy, SEC_LEN * 0.88, 0.05, band_h))
    ends = [(centers[0], -1), (centers[-1], 1)]
    for cx, sgn in ends:
        ex = cx + sgn * (SEC_LEN / 2 * 0.98)
        _append(verts, faces, _box(ex, 0, band_cy + 0.1, 0.05, WIDTH * 0.8, band_h * 1.05))
    return mf.new_mesh_object("glazing", verts, faces, mat)


def build_bellows(mat):
    """Articulation bellows hint: a narrower dark concertina block filling the
    gap between the two sections."""
    verts, faces = [], []
    top = HEIGHT - ROOF_RISE
    body_h = top - FLOOR
    centers = _section_centers()
    for a, b in zip(centers, centers[1:]):
        gx = (a + b) / 2
        _append(verts, faces,
                _box(gx, 0, FLOOR + body_h / 2, BELLOWS, WIDTH * 0.86, body_h * 0.92))
    return mf.new_mesh_object("bellows", verts, faces, mat)


def build_wheels(mat):
    """Small low-floor trucks below FLOOR — one under each section end pair,
    kept small so the low-floor read holds."""
    verts, faces = [], []
    for cx in _section_centers():
        for end in (-1, 1):
            wx = cx + end * SEC_LEN * 0.30
            _append(verts, faces, _box(wx, 0, FLOOR / 2, 1.6, WIDTH * 0.7, FLOOR))
    return mf.new_mesh_object("wheels", verts, faces, mat)


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
        out = "/tmp/tram.glb"

    mf.reset_scene()
    m_body = mf.palette_material("transit_body", "transit_body")
    m_glass = mf.palette_material("transit_glass", "transit_glass")
    m_roof = mf.palette_material("transit_roof", "transit_roof")

    build_bodies(m_body)
    build_roof(m_roof)
    build_glazing(m_glass)
    build_bellows(m_glass)
    build_wheels(m_glass)

    if preview:
        import turntable
        turntable.render_turntable(preview)
    if out:
        mf.export_glb(out)


main()
