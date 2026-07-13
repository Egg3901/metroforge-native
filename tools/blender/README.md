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
| `turntable.py` | **Critique renderer.** Given a `.glb` (or the in-memory scene) renders 6 flat-shaded views (front/side/3quarter/top + a game-camera oblique + a ~2km far view) to PNG in ~1.5s (Cycles-CPU, uniform light, neutral grey bg — no GPU/EGL needed). The vision-critique half of the loop. |
| `gen_bridge.py` | Suspension bridge family: `generic` (steel portal towers, 2 cables, no fan) + `brooklyn` (stone twin towers with TWO pointed gothic arches, 4-cable catenary + hanger curtain + signature diagonal stay fan + anchorages). |
| `gen_truss.py` | Generic Warren through-truss box (120–350m spans) so mid-length crossings stop using flat deck ribbons. |
| `gen_train.py` | 3-car metro consist (cab window, door pattern, continuous window band, visible bogies). |
| `gen_clouds.py` | 3–4 low-poly rounded cloud clumps. |
| `make-assets.sh` | Regenerates every `.glb` into `crates/mf-game/assets/models/`. `--preview` also renders a turntable sheet per asset into `previews/`. |
| `previews/` | Committed turntable sheets for the final iteration of each asset (the model-craft gate). |

## The model-craft critique loop (BINDING)

Adopted 2026-07-13 (ops-knowledge doc `metroforge-model-craft-method`) after the
pilot bridge was rejected. **Never export blind.** Every iteration of every
asset: regenerate → render the turntable → *read* the images → score a written
silhouette checklist pass/fail per item → edit the generator. Minimum 4
iterations per hero asset; stop at all-pass (or 8, reporting honestly). Each
generator takes `--preview <prefix>` to render its own turntable inline:

```bash
blender -b --factory-startup --python tools/blender/gen_bridge.py -- \
    brooklyn --preview /tmp/bk_iter7 /tmp/bk.glb
# render an existing .glb:
blender -b --factory-startup --python tools/blender/turntable.py -- \
    --glb crates/mf-game/assets/models/bridge_brooklyn.glb --out /tmp/bk
```

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
- `crates/mf-render/src/bridges.rs` places ONE bridge model per qualifying
  over-water span, picked by chord length (>350m → suspension family, the single
  longest getting the Brooklyn variant; 120–350m → truss; shorter → left as a
  ribbon). `roads.rs` calls the same `plan_bridge_placements` to suppress its
  flat deck slab/piers/shadow under a placed model (no double-render, #144).
- `crates/mf-render/src/vehicles.rs` swaps the metro brick for the consist
  model behind `METRO_MODEL_SWAP` (Medium+, metro only).

## Cel-outline compatibility (reported)

The inverted-hull outline pass (`outline.rs`) keys off procedurally built
meshes with handles we own. glTF scenes spawn their meshes asynchronously as
child entities we do not own, so applying the inverted hull cheaply is not
practical. **Models render without outlines** — their flat-shaded, low-poly
silhouette carries the cel read on its own.
