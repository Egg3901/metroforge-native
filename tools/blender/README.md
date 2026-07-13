# MetroForge scripted asset pipeline (`tools/blender/`)

Blender-scripted, **procedural** 3D assets for the game. Models are authored in
`bpy` Python (owner rule: no AI-generated art for real-world landmarks) and
exported to `.glb` for the Bevy renderer to load.

## Art direction (BINDING)

Mirror's Edge white cel city. Every model ships:

- **flat-shaded** (hard normals, no smooth groups),
- **low-poly**,
- **palette-material / vertex-colored** — near-white structure, black road
  deck, transit stays its route color; **no textures**.

Palette values mirror `crates/mf-render/src/palette.rs` (LIGHT theme). See
`mf_bpy/__init__.py` `PALETTE`.

## Layout

| File | Role |
|---|---|
| `mf_bpy/__init__.py` | Shared lib: palette materials, flat shading, deterministic mesh build, `.glb` export (1 unit : 1 m, +Y up = Bevy). |
| `gen_bridge.py` | Pilot A. Parametric suspension bridge family (`generic` + `brooklyn` variants). |
| `gen_train.py` | Pilot B. 3-car metro consist (window band, curved roof, bogie hint). |
| `gen_clouds.py` | Pilot C. 3–4 low-poly rounded cloud clumps. |
| `make-assets.sh` | Regenerates every `.glb` into `crates/mf-game/assets/models/`. |

## The loop

1. Edit a generator (or `mf_bpy`).
2. Regenerate all assets:
   ```bash
   ./tools/blender/make-assets.sh          # uses `blender` on PATH (5.x)
   BLENDER=/path/to/blender ./tools/blender/make-assets.sh
   ```
3. Commit **both** the `.py` generators and the regenerated `.glb` outputs.
   The `.glb` are committed artifacts; regeneration is **deterministic**
   (no RNG, no timestamps, floats rounded on build), so re-running produces
   byte-identical files — verified with `md5sum`.
4. Run a single asset by hand:
   ```bash
   blender --background --factory-startup --python tools/blender/gen_bridge.py -- brooklyn /tmp/b.glb
   ```

## Scale & orientation contract

- **1 Blender unit = 1 meter.** Export is `.glb`, `export_yup=True`, so Blender
  Z-up authoring becomes Bevy +Y-up at load.
- Bridge models face **+X along the span**, deck centered at the origin, deck
  top at y≈0. Bevy scales X to the real span, Z to the deck width.
- The metro consist runs along **+X**, base at y=0 (Bevy drops it onto the
  deck). Its **body material is near-white** so Bevy's per-route
  `StandardMaterial.base_color` tint reads through (windows/roof/bogies stay
  neutral).

## Bevy side

- `bevy_gltf` feature is enabled in the root `Cargo.toml`.
- `crates/mf-render/src/models.rs` loads the `.glb` scenes at startup into a
  `ModelHandles` resource and drives the cloud puffs.
- `crates/mf-render/src/bridges.rs` places suspension models on long over-water
  spans.
- `crates/mf-render/src/vehicles.rs` swaps the metro brick for the consist
  model behind `METRO_MODEL_SWAP` (Medium+, metro only).

## Cel-outline compatibility (reported)

The inverted-hull outline pass (`outline.rs`) keys off procedurally built
meshes with handles we own. glTF scenes spawn their meshes asynchronously as
child entities we do not own, so applying the inverted hull cheaply is not
practical. **Models render without outlines** — their flat-shaded, low-poly
silhouette carries the cel read on its own.
