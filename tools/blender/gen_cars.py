"""gen_cars.py — ambient street-traffic cars: 3 tiny variants, <300 tris each.

Run:  blender -b --factory-startup --python gen_cars.py -- <out.glb>
Add   --preview <prefix>  to render a turntable critique sheet.

These are BACKGROUND props for the ambient traffic system, NOT transit — so
they wear DESATURATED muted tones (car_body_a/b/c), never route colors. The
in-game traffic system (crates/mf-render/src/traffic.rs) instances matching
low-poly meshes procedurally for cheapness; this generator is the art record +
silhouette critique reference, and the three variants sit 6m apart on X.

Variants:
  sedan     — long low body + shallow greenhouse.
  hatchback — shorter body + taller stubby greenhouse to the tail.
  van       — tall boxy body + small front greenhouse only.

Silhouette checklist:
  * SIDE : each variant reads as a distinct car type; a greenhouse (cabin)
           set on a body; muted (not vivid) tone; wheels hinted.
  * <300 tris each.

Orientation: cars run along +X, base at z=0.
"""

from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

SPACING = 6.0  # variants sit this far apart on X for the turntable sheet


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


def _car(verts, faces, gverts, gfaces, ox, kind):
    """Append one car (body -> verts/faces, greenhouse -> gverts/gfaces).
    kind in {sedan, hatch, van}. Body sits with wheels hinted below z=0.3."""
    if kind == "sedan":
        L, W, H = 4.6, 1.9, 1.15
        floor = 0.35
        cab_l, cab_h, cab_x = L * 0.42, 0.72, -L * 0.05
    elif kind == "hatch":
        L, W, H = 3.9, 1.85, 1.2
        floor = 0.35
        cab_l, cab_h, cab_x = L * 0.5, 0.82, -L * 0.10
    else:  # van
        L, W, H = 4.9, 2.0, 1.55
        floor = 0.4
        cab_l, cab_h, cab_x = L * 0.34, 0.55, L * 0.28
    # body
    _append(verts, faces, _box(ox, 0, floor + H / 2, L, W, H))
    # greenhouse (cabin) — slightly narrower
    _append(gverts, gfaces, _box(ox + cab_x, 0, floor + H + cab_h / 2, cab_l, W * 0.86, cab_h))
    # wheel hint: a dark thin skirt below the body
    _append(gverts, gfaces, _box(ox, 0, floor * 0.5, L * 0.86, W * 1.02, floor * 0.6))


def build(kind_offsets, m_body, m_glass, name):
    verts, faces = [], []
    gverts, gfaces = [], []
    for ox, kind in kind_offsets:
        _car(verts, faces, gverts, gfaces, ox, kind)
    mf.new_mesh_object(f"{name}_body", verts, faces, m_body)
    mf.new_mesh_object(f"{name}_glass", gverts, gfaces, m_glass)


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
        out = "/tmp/cars.glb"

    mf.reset_scene()
    m_glass = mf.palette_material("car_glass", "car_glass")
    # Three variants, each its own muted body tone, laid out along X.
    build([(-SPACING, "sedan")], mf.palette_material("car_body_a", "car_body_a"),
          m_glass, "sedan")
    build([(0.0, "hatch")], mf.palette_material("car_body_b", "car_body_b"),
          m_glass, "hatch")
    build([(SPACING, "van")], mf.palette_material("car_body_c", "car_body_c"),
          m_glass, "van")

    if preview:
        import turntable
        turntable.render_turntable(preview)
    if out:
        mf.export_glb(out)


main()
