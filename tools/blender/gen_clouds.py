"""gen_clouds.py — low-poly rounded cloud puffs (Pilot C).

Run:  blender --background --python gen_clouds.py -- <out.glb>

Produces ONE .glb containing 3-4 distinct cloud clumps as separate named
meshes (puff_0..puff_3), each a cluster of low-subdivision icospheres merged
into a rounded clump. Flat white top, slightly grey underside (a thin flat
slab tucked under each clump). No textures.

Bevy instances these as drifting scenes at high altitude on Medium+.
Scale: roughly 120-260m across per clump (stylised, reads at city scale).
"""

from __future__ import annotations

import sys
import os

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mf_bpy as mf  # noqa: E402

# each clump: list of (x, y, z, radius) icosphere lobes (meters, Z-up)
CLUMPS = [
    [(0, 0, 0, 60), (55, 0, 8, 42), (-50, 0, 6, 46), (18, 0, 22, 34)],
    [(0, 0, 0, 74), (70, 0, -4, 40), (-64, 0, 4, 44)],
    [(0, 0, 0, 50), (40, 0, 10, 38), (-38, 0, 8, 40), (0, 0, 20, 30), (78, 0, 0, 26)],
    [(0, 0, 0, 66), (58, 0, 6, 36), (-54, 0, 2, 38), (10, 0, 24, 28)],
]


def build_clump(idx, lobes, m_top, m_under):
    parts = []
    minx = min(l[0] - l[3] for l in lobes)
    maxx = max(l[0] + l[3] for l in lobes)
    miny = min(l[1] - l[3] for l in lobes)
    maxy = max(l[1] + l[3] for l in lobes)
    for (x, y, z, r) in lobes:
        obj = mf.add_primitive_flat(
            "ico_sphere", m_top, f"lobe_{idx}",
            subdivisions=1, radius=r, location=(x, y, z + r * 0.3),
        )
        # squash vertically a touch so clumps read flat/rounded, not spherical
        obj.scale = (1.0, 1.0, 0.62)
        parts.append(obj)
    # grey underside slab
    slab = mf.add_primitive_flat(
        "cube", m_under, f"under_{idx}",
        size=1.0, location=((minx + maxx) / 2, (miny + maxy) / 2, 0.0),
    )
    slab.scale = ((maxx - minx) / 2 * 0.9, (maxy - miny) / 2 * 0.9, 3.0)
    parts.append(slab)
    clump = mf.join_objects(parts, f"puff_{idx}")
    # move clump to its own origin so instances place cleanly
    clump.location = (idx * 600.0, 0.0, 0.0)
    return clump


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
        out = "/tmp/clouds.glb"

    mf.reset_scene()
    m_top = mf.palette_material("cloud", "cloud")
    m_under = mf.palette_material("cloud_under", "cloud_under")

    for i, lobes in enumerate(CLUMPS):
        build_clump(i, lobes, m_top, m_under)

    if preview:
        import turntable
        turntable.render_turntable(preview)
    mf.export_glb(out)


main()
