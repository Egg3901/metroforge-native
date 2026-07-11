# Contributing

House patterns for PRs in `metroforge-native`. Prefer matching existing code
over inventing parallel styles. CI gates (from `docs/DEVELOPMENT.md` /
`.github/workflows/ci.yml`):

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Wire and architecture references: [`docs/PROTOCOL.md`](docs/PROTOCOL.md),
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

---

## SettingsControls param bundling

When an egui system needs several related `ResMut`s, bundle them with
`#[derive(SystemParam)]` so the system stays under Bevy's parameter limit.

Canonical example — `crates/mf-game/src/hud.rs`:

```rust
#[derive(SystemParam)]
struct SettingsControls<'w> {
    quality: ResMut<'w, QualityTier>,
    theme: ResMut<'w, Theme>,
    weather: ResMut<'w, WeatherEffects>,
    config: ResMut<'w, MfConfig>,
}
```

Used by the title Settings screen, in-game settings, and pause overlay. Add
new Settings-owned resources to this struct (or a sibling `SystemParam`)
instead of lengthening every call site's parameter list.

---

## Serde-default config fields

Persistent TOML (`config.rs`, and the same idea in `goals.rs`) must keep old
files loadable when new keys appear:

- `#[serde(default)]` (or `default = "fn_name"`) on every new field.
- Prefer `skip_serializing_if` so defaults stay out of rewritten files when
  appropriate.
- Provide an explicit `Default` impl / default fns that match the serde
  defaults.
- Add a unit test that deserializes a **legacy** snippet missing the new key
  (see `legacy_config_without_weather_defaults_on` in `config.rs`).

Do not require players to hand-edit `config.toml` after an upgrade.

---

## Quantized day/night gating

`DayNightState` is smoothed every frame. Do not gate GPU writes on
`Res<T>::is_changed()` alone for continuously drifting values — that dirties
uniforms forever.

Pattern in `crates/mf-render/src/daynight.rs`:

- Quality: if `!day_night_enabled` (Potato), pin noon / `night_factor = 0`.
- Apply path: quantize `night_factor` to 1/256 and hour to 1/1024 of a day;
  skip clear/ambient/sun writes when buckets are unchanged (unless quality or
  theme changed).

Same discipline elsewhere: building night dim (`quantize_night_factor` 1/256),
reveal uniforms (0.5 m / 1/64), transit crowding (1/64), vehicle brightness
(1/64). If a value drifts continuously, bucket it before touching `Assets`.

---

## Paint-key material caching

For many similar meshes (vehicles), do **not** mutate a per-entity material
every tick. Share materials by a discrete paint key and swap
`MeshMaterial3d` handles when the key changes.

Canonical example — `crates/mf-render/src/vehicles.rs`:

- Body key: `(color_idx, brightness_bucket, unlit, overlay_dimmed)`.
- Light key: `(LightKind, night_bucket, unlit)`.
- Cache: `HashMap<Key, Handle<StandardMaterial>>`.
- Wire `colorTable` hex values are ignored; index the client vivid palette by
  `routeColorIdx`.

When adding a new visual that repeats across entities, extend this pattern
rather than per-slot `materials.get_mut` churn.

---

## Rebuild signatures

Static layers (`MfRenderSet::Statics` / Terrain) must cache-check every frame
and rebuild only when a structural signature changes. Store the signature on
a private `Resource`, compare, early-return, else despawn and rebuild.

Examples:

| Layer | File | Signature shape |
|---|---|---|
| Terrain | `terrain.rs` | `(fields.version, subdiv, theme, shader_water)` |
| Roads | `roads.rs` | `(fields.version, len, points, theme, densify_bits)` |
| Buildings | `buildings.rs` | `rebuild_key(...)` + theme |
| Transit | `transit.rs` | hashed `u64` of structural UI ⊕ knobs |
| Trees / lamps | `trees.rs` / `street_lamps.rs` | version + theme + enable knobs |

Include every input that affects baked vertex color or topology (theme,
quality densify, fields version). Prefer one `rebuild_key` / `signature`
definition over ad hoc multi-field compares at call sites.

---

## No em/en dashes in player copy

Player-facing strings must not contain ASCII `-`, en dash (U+2013), or em
dash (U+2014). Prefer plain sentences or spaced words.

Enforced by:

- Unit tests: `campaign::describe_goal_is_dash_free`,
  `report_ui::verdict_heading_is_dash_free_for_every_shown_outcome`.
- Comments on tutorial / campaign / panels copy.
- `scripts/package.sh` strips en/em dashes from packaged READMEs.

When you add HUD/toast/report strings, add or extend a dash-free assertion
if the string is generated from an enum or table. Do not rely on `Debug`
formatting for player text.

---

## Fan-out worktrees + shared `CARGO_TARGET_DIR`

Large waves land as parallel feature worktrees that avoid colliding on
hotspot files (`main.rs` plugin tuples, `input.rs` keybinds). Module headers
in `build_ui.rs`, `overlays.rs`, `map_mode.rs`, `attract.rs`, and `panels.rs`
document that split: own a new module, leave integration stubs, resolve
keybind / `.add_plugins` conflicts at merge time.

When using multiple git worktrees on this workspace:

1. Give each feature its own worktree/branch (`cursor/...` naming for agent
   branches; otherwise follow team branch policy).
2. Point every worktree at one shared target directory so dependencies are
   not rebuilt N times:

   ```sh
   export CARGO_TARGET_DIR=/path/to/shared/target
   ```

3. Keep crate dependency direction intact: **render never depends on game**;
   put shared resources in `mf-state`.
4. Do not "temporarily" edit the same hotspot file from two worktrees;
   leave an integration handoff comment instead.

---

## Protocol and docs changes

- Additive JSON fields: `#[serde(default)]` / `Option`; do not bump
  `PROTOCOL_VERSION` (see [`docs/PROTOCOL.md`](docs/PROTOCOL.md) §5).
- Additive binary: follow `StaticBuildings` (optional msgType; wire version
  bump only when layout changes within that msgType).
- Breaking wire changes: bump `PROTOCOL_VERSION` in `mf-protocol` and the
  sidecar together; update `docs/PROTOCOL.md` in the same PR.
- `mf-protocol` and `mf-state` enable `#![warn(missing_docs)]` — every new
  `pub` item needs a doc comment.

---

## PR labeling

`.github/release.yml` groups release notes by label (Features, Rendering,
Simulation & Protocol, Fixes, Performance, Other). Label before merge.
