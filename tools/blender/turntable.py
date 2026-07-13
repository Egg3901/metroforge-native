"""turntable.py — headless multi-view critique renderer for the model-craft loop.

Renders a .glb (or the current in-memory scene) to 6 flat-shaded PNG views so
the agent can READ the images and score a silhouette checklist BEFORE exporting
blind. This is the mandatory critique half of the render-critique-revise loop
(ops-knowledge doc `metroforge-model-craft-method`).

Views:
  front       — looking down +? at the span face (the money shot for arches)
  side        — looking along the span axis (catenary / truss profile)
  3quarter    — 45deg oblique, elevated
  top         — plan view
  game_600m   — perspective at a ~600m-equivalent oblique (game-camera feel)
  far_2km     — perspective at a ~2km-equivalent distance (does it still read?)

Renderer: BLENDER_WORKBENCH, FLAT lighting, MATERIAL color (reads each object's
palette diffuse_color), neutral grey background. Fast (sub-second per view).

Usage (standalone, loads a glb):
    blender -b --factory-startup --python tools/blender/turntable.py -- \
        --glb path/to/model.glb --out tools/blender/previews/<name>

In-generator (render the scene you just built, no export):
    import turntable; turntable.render_turntable("tools/blender/previews/<name>")
"""

from __future__ import annotations

import math
import os
import sys

import bpy  # type: ignore
from mathutils import Vector  # type: ignore

# neutral grey the cel-white structure and near-black deck both read against
BG_GREY = (0.32, 0.33, 0.34)
RES = (640, 480)


def _scene_bounds():
    """World-space AABB of all mesh objects. Returns (center, size) Vectors."""
    lo = Vector((1e18, 1e18, 1e18))
    hi = Vector((-1e18, -1e18, -1e18))
    found = False
    for obj in bpy.data.objects:
        if obj.type != "MESH":
            continue
        found = True
        for corner in obj.bound_box:
            w = obj.matrix_world @ Vector(corner)
            for i in range(3):
                lo[i] = min(lo[i], w[i])
                hi[i] = max(hi[i], w[i])
    if not found:
        return Vector((0, 0, 0)), Vector((1, 1, 1))
    return (lo + hi) * 0.5, (hi - lo)


def _setup_render():
    """Cycles-CPU, uniform-environment lighting so it renders headless with no
    GPU/EGL. A flat white world lit from all directions gives a near-flat cel
    read (facets visible, no harsh shadows), and every material's base_color
    reads true — good enough for silhouette critique and it never needs a GPU."""
    scene = bpy.context.scene
    scene.render.engine = "CYCLES"
    scene.cycles.device = "CPU"
    scene.cycles.samples = 8
    scene.cycles.use_denoising = False
    scene.render.resolution_x, scene.render.resolution_y = RES
    scene.render.resolution_percentage = 100
    scene.render.film_transparent = False
    # neutral grey world providing uniform ambient light + background
    world = bpy.data.worlds.get("MFWorld") or bpy.data.worlds.new("MFWorld")
    world.use_nodes = True
    bg = world.node_tree.nodes.get("Background")
    if bg:
        bg.inputs["Color"].default_value = (*BG_GREY, 1.0)
        bg.inputs["Strength"].default_value = 1.4
    scene.world = world


def _make_cam():
    cam = bpy.data.cameras.new("mf_turntable_cam")
    obj = bpy.data.objects.new("mf_turntable_cam", cam)
    bpy.context.collection.objects.link(obj)
    bpy.context.scene.camera = obj
    return obj, cam


def _point_at(cam_obj, cam_loc, target):
    cam_obj.location = cam_loc
    direction = (target - cam_loc)
    cam_obj.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()


def render_turntable(out_prefix: str) -> None:
    """Render the current scene's 6 critique views to <out_prefix>_<view>.png."""
    os.makedirs(os.path.dirname(out_prefix) or ".", exist_ok=True)
    _setup_render()
    bpy.context.view_layer.update()
    center, size = _scene_bounds()
    print(f"[turntable] bounds center={tuple(round(c,1) for c in center)} "
          f"size={tuple(round(s,1) for s in size)}")
    span = max(size.x, size.y)  # longest horizontal extent ~ the real span

    cam_obj, cam = _make_cam()
    cam.type = "PERSP"
    cam.lens_unit = "FOV"
    cam.clip_start = 1.0
    cam.clip_end = 100000.0
    fov = math.radians(42.0)

    def fit_dist(w, h, margin=1.18):
        """Distance so a w x h frame fills a `fov` perspective."""
        half = max(w, h * 640 / 480) * 0.5
        return half / math.tan(fov / 2) * margin

    # Each view: (direction from center to camera, framed extents (w,h)).
    # `front` looks DOWN THE SPAN AXIS (+X) at the tower face — this is where the
    # two side-by-side pointed arches read (the deck passes through them). `side`
    # is the long profile elevation (catenary + truss band).
    half = size * 0.5
    views = {
        # (dir, framed extents, fixed-dist, half-depth along the view axis)
        "front":    (Vector((1.0, 0.0, 0.06)).normalized(), (size.y, size.z), None, half.x),
        "side":     (Vector((0.0, -1.0, 0.06)).normalized(), (size.x, size.z), None, half.y),
        "3quarter": (Vector((0.75, -0.65, 0.42)).normalized(), (span, span * 0.6), None, span * 0.5),
        "top":      (Vector((0.0, -0.001, 1.0)).normalized(), (size.x, size.y), None, half.z),
        "game_600m": (Vector((0.55, -0.72, 0.42)).normalized(), None, max(span * 0.9, 600.0), 0.0),
        "far_2km":  (Vector((0.42, -0.80, 0.40)).normalized(), None, max(span * 3.0, 2000.0), 0.0),
    }

    for name, (dir_vec, frame, fixed, depth) in views.items():
        if fixed is not None:
            dist = fixed
            cam.angle = math.radians(20.0) if name == "far_2km" else math.radians(42.0)
        else:
            cam.angle = fov
            # push the camera clear of the model's own depth along the view axis
            dist = fit_dist(frame[0], frame[1]) + depth
        _point_at(cam_obj, center + dir_vec * dist, center)
        bpy.context.scene.render.filepath = f"{out_prefix}_{name}.png"
        bpy.ops.render.render(write_still=True)
        print(f"[turntable] {out_prefix}_{name}.png")


def _load_glb(path: str) -> None:
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=path)


def main() -> None:
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    glb = None
    out = "/tmp/turntable"
    i = 0
    while i < len(argv):
        if argv[i] == "--glb":
            glb = argv[i + 1]
            i += 2
        elif argv[i] == "--out":
            out = argv[i + 1]
            i += 2
        else:
            i += 1
    if glb:
        _load_glb(glb)
    render_turntable(out)


if __name__ == "__main__":
    main()
