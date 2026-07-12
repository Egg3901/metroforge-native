# Architecture

MetroForge Desktop (`metroforge-native`) is a Rust/Bevy renderer and input layer
in front of a simulation it does not own. Economy, routing, demand, and fail
conditions live in the TypeScript sim core (in-repo under [`sim/`](../sim/)), reached
through the wire protocol in [`PROTOCOL.md`](PROTOCOL.md). This client connects,
mirrors state, draws it, and forwards player commands. No sim rules are
reimplemented here.

---

## Crate map

```
mf-protocol   (no Bevy)
mf-net        (bevy_app / bevy_ecs)
mf-state      (bevy_app / bevy_ecs / bevy_math; depends on mf-net + mf-protocol)
mf-render     (full bevy; depends on mf-state + mf-protocol)
mf-game       (full bevy + egui; binary `metroforge`; depends on all of the above)
```

### Dependency rule

Dependencies flow **one way toward `mf-game`**:

```
mf-protocol
    ↑
 mf-net ──────────────┐
    ↑                 │
 mf-state ←───────────┘
    ↑
 mf-render
    ↑
 mf-game
```

**`mf-render` never depends on `mf-game`.** Confirmed in
`crates/mf-render/Cargo.toml` (only `mf-state`, `mf-protocol`, `bevy`). Shared
toggles that both shells need (`SubwayView`, `OverlayState`, `RevealState`,
`Theme`, `QualityTier`, `HeightAt`) live in `mf-state` so render can read them
without importing the game shell. Comments in `roads.rs` / `overlays.rs`
restate this edge explicitly.

`mf-state` **does** depend on `mf-net` (for `SimEvent` and `NetSet::Drain`
ordering in `plugin.rs`).

### Responsibilities

| Crate | Owns | Key paths |
|---|---|---|
| **mf-protocol** | Serde JSON mirrors + binary codec; `PROTOCOL_VERSION`; `FromSimMsg` | `types.rs`, `envelope.rs`, `binary.rs` |
| **mf-net** | `SimTransport`, `WsTransport`, `SidecarProcess`, ping/liveness/reconnect | `transport.rs`, `ws_transport.rs`, `sidecar.rs`, `reconnect.rs`, `plugin.rs` |
| **mf-state** | Shared `Resource`s filled from `SimEvent` | `city.rs`, `fields.rs`, `frame.rs`, `ui.rs`, `quality.rs`, … |
| **mf-render** | 3D layers under `MfRenderPlugin` / `MfRenderSet` | `lib.rs` + per-layer modules |
| **mf-game** | App state machine, camera, input→commands, egui HUD, config, campaign | `state.rs`, `hud.rs`, `config.rs`, `campaign.rs` |

---

## mf-protocol

Pure wire types. No Bevy. Two halves unified as `FromSimMsg`:

- JSON: `ToSim` / `FromSimJson` + payload structs.
- Binary: `FrameSnapshot`, `Fields`, `Traffic`, `StaticMask`, `StaticBuildings`.

`Frame` / `Fields` variants wrap payloads in `Arc` so `mf-state` can retain
"latest" without deep-cloning ~20 Hz / 7-day arrays.

Full contract: [`PROTOCOL.md`](PROTOCOL.md).

---

## mf-net

Only crate allowed to know the sim is a separate OS process today.

```rust
pub trait SimTransport: Send + Sync {
    fn send(&self, msg: ToSim) -> anyhow::Result<()>;
    fn try_recv(&self) -> Option<FromSimMsg>;
    fn is_alive(&self) -> bool;
}
```

`WsTransport` runs blocking `tungstenite` on a background thread; two
crossbeam channels bridge to ECS. `drain_inbound_system` pushes
`Events<SimEvent>` each frame. Ping every 5 s; 10 s silence → dead; reconnect
policy in `reconnect.rs` (500 ms → 4 s, 5 attempts).

`SidecarProcess` locates and spawns the sidecar binary (lookup order: env var, then
next to the running executable, then a dev fallback of `bun run sidecar/index.ts`
against the in-repo `sim/` package), parses its one-line stdout handshake to
learn the assigned port, captures a rolling stderr log tail, and kills the process
group on `Drop`. On Unix the child sets `PR_SET_PDEATHSIG` so a client crash cannot
leave a zombie sidecar; on Windows the child is assigned to a Job Object with
`KILL_ON_JOB_CLOSE`. Stale `metroforge-sidecar` processes from a previous run are
reaped on every spawn. `reconnect.rs` implements the liveness policy: process exit
**or** no inbound traffic for 5 seconds means the sim is declared dead (the two
causes are distinguished); respawn and reconnect with backoff from 500 ms up to 4 s,
for up to 3 attempts. Mid-game recovery re-handshakes and restores from autosave
without returning to MainMenu; exhausting attempts surfaces a diagnostics screen
with the sidecar log tail. Sidecar spawn / `--port 0` / `$MF_SIDECAR_PATH` rules:
[`PROTOCOL.md` §4](PROTOCOL.md).

---

## mf-state

Resources filled by `apply_sim_events_system` (after `NetSet::Drain`):

| Resource | Source | Cadence / notes |
|---|---|---|
| `CurrentCity` | `ready` + `StaticMask` + optional `StaticBuildings` | Masks gate Loading; buildings do not |
| `LatestFields` | msgType=2 | Init + every 7 sim-days; `Arc` |
| `LatestFrame` | msgType=1 | Every 50 ms (~20 Hz); `Arc`; no interpolation buffer |
| `LatestUi` | `t:"ui"` | 2 Hz |
| `LatestDemand` | `t:"demand"` | Assignment-driven (see `demand.rs`) |
| `QualityTier` | Boot / HUD | Knob table via `knobs()` |
| `Theme` | Boot / HUD | Consumed by `palette.rs` |
| `SubwayView` | Input flips `active`; render steps `t` | |
| `HeightAt` | Default flat 0; terrain replaces sampler | |
| `RevealState` | Game input drives; render copies to shader | |
| `OverlayState` | Game cycles; render dims transit | |
| `WeatherEffects` | Settings checkbox | Gated by quality atmosphere knob |

`Traffic` and control-plane JSON (`commandResult`, `toast`, …) are **not**
mirrored here; consumers read `SimEvent` directly (`plugin.rs`).

---

## mf-render — pipeline stages

`MfRenderSet` chain (`lib.rs`):

```
sync_theme_system  (before Terrain)
    → Terrain
    → Statics
    → Dynamic
```

| Set | Systems (representative) | Rebuild policy |
|---|---|---|
| **Terrain** | `build_terrain_system`, terrain material quality | Rebuild on signature change; owns `HeightAt` replacement |
| **Statics** | roads, buildings, transit, trees, street_lamps | Cache-check every frame; rebuild only on signature/key change |
| **Dynamic** | vehicles, agents, daynight, atmosphere, subway, sky, water uniforms, outline, MSAA/fog/bloom | Per-frame (with quantized early-outs) |

### Rebuild-signature pattern

Each static layer stores a key and early-returns when unchanged:

| Layer | Signature (from code) |
|---|---|
| Terrain | `(fields.version, subdiv_divisor, theme, shader_water)` |
| Roads | `(fields.version, roads.len(), total_points, theme, densify_step_bits)` |
| Buildings | `rebuild_key(fields.version, buildings_count)` + `theme` |
| Transit | `u64` hash of structural `UiState` ⊕ densify ⊕ theme ⊕ unlit |
| Trees | `(fields.version, theme, tree_enabled)` |
| Street lamps | `(fields.version, roads.len(), total_points, theme, enabled, densify_bits)` |

Buildings geometry paths (`buildings.rs`): real `StaticBuildings` footprints →
mask → procedural density. Late-arriving footprints force one extra rebuild.

Vehicles: grow-only entity pool; materials shared by paint key
`(color_idx, brightness_bucket, unlit, overlay_dimmed)` (`vehicles.rs`).

### Quality knob flow

1. `mf-game/quality_boot.rs`: `config.toml` → `MF_QUALITY` → GPU `detect()` →
   default `Medium`.
2. `QualityTier` resource; HUD can change it at runtime.
3. `QualityTier::knobs()` → `QualityKnobs` (`mf-state/src/quality.rs`).
4. Render-global: MSAA, shadow map size, fog, bloom (`lib.rs`).
5. Per-layer: materials, draw distances, agent cap, terrain subdiv, day/night,
   ribbons, trees, water tier, street lamps.

---

## mf-game — shell and sim-facing behavior

### App state machine (`state.rs`)

```
Boot → ConnectingSim → MainMenu → Loading → InGame
                                                 ↓
                                             SimError  (exhausted sidecar reconnects)
```

- **Boot**: load config, spawn sidecar + WS.
- **ConnectingSim**: client `hello`; version check; store `SimHello`.
- **MainMenu**: city pick / load / settings (`MenuScreen` resource).
- **Loading**: send `init` (or save load); gate
  `masks_complete && fields && ui` → InGame.
- **Fatal net**: any non-Boot/MainMenu state → MainMenu.

### Tick model (as mirrored by this client)

| Signal | Cadence | Source in this repo |
|---|---|---|
| Sim frame snapshot | 50 ms (~20 Hz) | `binary.rs`, `frame.rs` |
| UI state | 2 Hz | `ui.rs`, `types.rs` |
| Fields | init + every 7 sim-days | `fields.rs` |
| Day length | `TICKS_PER_DAY = 1200` | `UiState::display_hour` |
| Demand | ~assignment interval (300 ticks / dirty) | `demand.rs` comments |
| Client render | Bevy frame rate (60+) | camera / vehicles comments |
| WS poll | 4 ms | `ws_transport.rs` |

### Determinism

This client does not run the sim. Determinism of gameplay outcomes is a
property of the sidecar: `(seed, command stream)` → state, verified there via
`requestReplay` / `stateHash` (types in `ReplayPayload`). Client-side
"determinism" comments cover only local cosmetics (audio LFSR, tree jitter
hashes, verify-harness fixed cursor) — not economy or routing.

`init` seeds from wall-clock nanos (`state.rs` `rand_seed`); replays use the
recorded seed from `ReplayPayload`.

### Scenario layer

**Wire:** `ScenarioRules` on `InitPayload.rules` / `ReplayPayload.rules`
(`types.rs`). Current init paths in `state.rs` / `attract.rs` pass
`rules: None`.

**Client campaign** (`campaign.rs`): stars, unlocks, and end-of-scenario
outcomes evaluated over `LatestUi` / `UiState` (including sidecar-provided
`failed`, `max_day`, `bankrupt`). The sidecar is not required to know about
stars; the client adds that layer. Separate alpha goals live in `goals.rs`.

---

## Coordinate conventions

World X → Bevy X; world Y → Bevy Z; up is +Y. Units are meters. Ground is the
XZ plane; origin is city center. Wire `(x, y)` becomes Bevy
`(x, heightAt(x, y), y)` plus per-layer vertical offsets.

---

## Related docs

- Wire contract: [`PROTOCOL.md`](PROTOCOL.md)
- Dev/CI/release: [`DEVELOPMENT.md`](DEVELOPMENT.md)
- PR house patterns: [`../CONTRIBUTING.md`](../CONTRIBUTING.md)
