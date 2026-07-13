"""mf_bpy — shared Blender/bpy helpers for the MetroForge scripted asset pipeline.

Art direction (BINDING, mirrors crates/mf-render/src/palette.rs): Mirror's Edge
white cel city. Models ship FLAT-SHADED, LOW-POLY, palette-material / vertex
colored. Near-white structure, black road deck, transit keeps its route color.
NO textures.

Conventions guaranteed by this lib:
  * Scale is 1 Blender unit : 1 meter.
  * Export is glTF binary (.glb), +Y up (Bevy convention).
  * Materials are unlit-friendly flat colors (metallic 0, roughness 1). Bevy
    loads them as StandardMaterial; the render tiers that want unlit read the
    base_color and drop the lighting, so keeping metallic/roughness flat means
    the same asset reads correctly on both lit (Medium/High) and unlit tiers.
  * Deterministic: no randomness, no timestamps. Re-running a generator
    produces byte-comparable geometry (floats are rounded on build).
"""

from __future__ import annotations

import math
import bpy  # type: ignore

# --- palette (sRGB 8-bit, mirrored from palette.rs LIGHT theme) --------------
# Structure reads near-white; the deck reads near-black; transit is tinted at
# runtime by Bevy (per-route), so the train BODY material is a neutral light
# grey that accepts a multiplicative tint cleanly.
PALETTE = {
    "structure":      (0xf4, 0xf5, 0xf2),  # building_top — primary white mass
    "structure_side": (0xe2, 0xe5, 0xe3),  # building_side — subtle facet shade
    "structure_base": (0xd6, 0xda, 0xd8),  # building_base — grounding shade
    "stone":          (0xea, 0xe9, 0xe4),  # bridge towers (warm off-white)
    "stone_shade":    (0xd2, 0xd3, 0xcd),  # tower facet shade
    "deck":           (0x17, 0x18, 0x1c),  # road — black road deck
    "deck_edge":      (0x2a, 0x2c, 0x32),  # road_edge — kerb / parapet
    "cable":          (0x3a, 0x3d, 0x44),  # dark grey suspension cable/stay
    "transit_body":   (0xf2, 0xf3, 0xf1),  # near-white; runtime route tint x this
    "transit_glass":  (0x2a, 0x2f, 0x38),  # dark window band
    "transit_roof":   (0xcf, 0xd3, 0xd6),  # slightly greyer roof
    "cloud":          (0xff, 0xff, 0xff),  # flat white cloud top
    "cloud_under":    (0xcf, 0xd4, 0xd9),  # slight grey underside
}


def _srgb_to_linear(c: float) -> float:
    c /= 255.0
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def linear_rgba(name: str, alpha: float = 1.0):
    r, g, b = PALETTE[name]
    return (_srgb_to_linear(r), _srgb_to_linear(g), _srgb_to_linear(b), alpha)


def reset_scene() -> None:
    """Wipe the default scene to a clean, deterministic slate."""
    bpy.ops.wm.read_factory_settings(use_empty=True)


def palette_material(name: str, palette_key: str, alpha: float = 1.0):
    """Flat palette material. Metallic 0 / roughness 1 → reads flat on lit
    tiers and its base_color survives cleanly on unlit tiers."""
    mat = bpy.data.materials.get(name)
    if mat is None:
        mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    mat.diffuse_color = linear_rgba(palette_key, alpha)  # viewport / fallback
    nt = mat.node_tree
    bsdf = nt.nodes.get("Principled BSDF")
    if bsdf:
        bsdf.inputs["Base Color"].default_value = linear_rgba(palette_key, alpha)
        bsdf.inputs["Metallic"].default_value = 0.0
        bsdf.inputs["Roughness"].default_value = 1.0
        if "Specular IOR Level" in bsdf.inputs:
            bsdf.inputs["Specular IOR Level"].default_value = 0.0
    if alpha < 1.0:
        mat.blend_method = "BLEND"
    return mat


def flat_shade(obj) -> None:
    """Force flat shading (hard normals) — the cel look. No smooth groups."""
    for poly in obj.data.polygons:
        poly.use_smooth = False


def new_mesh_object(name: str, verts, faces, material):
    """Build a mesh object from raw verts/faces, assign one material, flat-shade.
    Verts are (x, y, z) in Blender Z-up meters; glTF export flips to Y-up."""
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata([tuple(round(c, 4) for c in v) for v in verts], [], faces)
    mesh.update()
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.collection.objects.link(obj)
    obj.data.materials.append(material)
    flat_shade(obj)
    return obj


def add_primitive_flat(kind: str, material, name: str, **kwargs):
    """Add a bpy primitive (cube/cylinder/ico_sphere/cone), assign a flat
    palette material, and flat-shade it. kwargs pass through to the op."""
    ops = {
        "cube": bpy.ops.mesh.primitive_cube_add,
        "cylinder": bpy.ops.mesh.primitive_cylinder_add,
        "ico_sphere": bpy.ops.mesh.primitive_ico_sphere_add,
        "cone": bpy.ops.mesh.primitive_cone_add,
    }
    ops[kind](**kwargs)
    obj = bpy.context.active_object
    obj.name = name
    obj.data.materials.clear()
    obj.data.materials.append(material)
    flat_shade(obj)
    return obj


def join_objects(objs, name: str):
    """Join a list of objects into the first, rename the result."""
    for o in bpy.context.selected_objects:
        o.select_set(False)
    for o in objs:
        o.select_set(True)
    bpy.context.view_layer.objects.active = objs[0]
    bpy.ops.object.join()
    result = bpy.context.active_object
    result.name = name
    return result


def export_glb(path: str) -> None:
    """Export the whole scene as .glb at 1 unit:1m, +Y up (Bevy).
    Deterministic: no draco, no textures/images, apply transforms."""
    for o in bpy.data.objects:
        o.select_set(True)
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format="GLB",
        export_yup=True,          # Bevy +Y up
        export_apply=True,        # bake modifiers/transforms
        export_texcoords=False,
        export_normals=True,
        export_materials="EXPORT",
        export_cameras=False,
        export_lights=False,
        export_extras=False,
        export_animations=False,
    )
    print(f"[mf_bpy] exported {path}")
