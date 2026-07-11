# Architecture

MetroForge Desktop is a Rust/Bevy renderer and input layer sitting in front of a
simulation it does not own. Every rule about how a city grows, how budgets balance,
how passengers route, and how a game ends lives in the existing deterministic
TypeScript sim core (`metroforge/src/core/`). This client's job is: connect to that
sim, mirror its state, and draw it in 3D. No game logic is duplicated here.

## Why sidecar, not rewrite

The obvious alternative to what's here was porting the sim to Rust. That was
rejected: it would mean two independent implementations of the same economy,
routing, and demand model, which inevitably drift apart under maintenance, and a
years-old bug class (client says one thing, server-authoritative logic says another)
that this project has no reason to invite. Instead, the sidecar wraps the *existing*
sim loop (`sim.worker.ts`, unmodified in its core logic) behind a small process
boundary and speaks a wire protocol shaped like a future native FFI call. One sim
implementation serves the web build, this desktop client, and any future client,
forever in lockstep.

The cost of this choice is a process boundary and a wire protocol (see
[`PROTOCOL.md`](PROTOCOL.md)) instead of a function call. That cost is paid once,
here, rather than paid continuously as two sims silently diverge.

## Crate-by-crate

```
mf-protocol   (no Bevy dependency)
mf-net        (Bevy: Resource/Event only)
mf-state      (Bevy: Resource/Event only)
mf-render     (Bevy: full)
mf-game       (Bevy: full; binary `metroforge`)
```

Dependencies flow one way: `mf-game` depends on `mf-net`, `mf-state`, and
`mf-render`; `mf-render` and `mf-net` both depend on `mf-state` and `mf-protocol`;
nothing depends on `mf-game`. This is what lets `mf-render` (owned by a separate
agent/session in this project's workflow) be developed against `mf-state`'s shared
resources without ever depending on, or being depended on by, `mf-game` directly.

### mf-protocol

Pure serde mirrors of every type that crosses the wire, plus the binary frame codec.
Deliberately has no Bevy dependency: it is the one crate that could be reused by a
non-Bevy consumer (a headless test harness, a future server, a different engine) with
zero changes. Two halves:

- `types.rs` / `envelope.rs`: the JSON control channel. `Command`, `UiState`,
  `StaticCityJson`, and every other JSON-shaped type, plus the `{t, seq?, p?}`
  envelope and the `ToSim` / `FromSimJson` message enums.
- `binary.rs`: the four binary hot-path frame layouts (`FrameSnapshot`, `Fields`,
  `Traffic`, `StaticMask`). Scalars are read via `from_le_bytes`; arrays are always
  copied out through `chunks_exact(4)` rather than cast in place, because an inbound
  WebSocket buffer is not guaranteed 4-byte aligned.

`lib.rs` unifies both halves into one `FromSimMsg` enum so downstream crates funnel
every inbound message, JSON or binary, through a single event type.

Full wire reference: [`PROTOCOL.md`](PROTOCOL.md).

### mf-net

Owns the fact that the sim is reachable at all, and is the *only* crate allowed to
know it's a separate OS process today. Its central seam is one trait:

```rust
pub trait SimTransport: Send + Sync {
    fn send(&self, msg: ToSim) -> anyhow::Result<()>;   // non-blocking enqueue
    fn try_recv(&self) -> Option<FromSimMsg>;            // non-blocking drain
    fn is_alive(&self) -> bool;
}
```

Bevy isn't tokio-native, so the concrete desktop implementation (`WsTransport`) runs
a blocking `tungstenite` WebSocket client on a dedicated background thread and
bridges it into the ECS through two `crossbeam-channel`s (outbound queue, inbound
queue). A Bevy system (`drain_inbound_system`) drains the inbound channel into
`Events<SimEvent>` once per frame; nothing in Bevy-land ever touches a socket
directly.

`SidecarProcess` locates and spawns the sidecar binary (lookup order: env var, then
next to the running executable, then a dev fallback of `bun run sidecar/index.ts`
against the sibling `metroforge` checkout), parses its one-line stdout handshake to
learn the assigned port, captures a rolling stderr log tail, and kills the process
group on `Drop`. On Unix the child sets `PR_SET_PDEATHSIG` so a client crash cannot
leave a zombie sidecar; on Windows the child is assigned to a Job Object with
`KILL_ON_JOB_CLOSE`. Stale `metroforge-sidecar` processes from a previous run are
reaped on every spawn. `reconnect.rs` implements the liveness policy: process exit
**or** no inbound traffic for 5 seconds means the sim is declared dead (the two
causes are distinguished); respawn and reconnect with backoff from 500 ms up to 4 s,
for up to 3 attempts. Mid-game recovery re-handshakes and restores from autosave
without returning to MainMenu; exhausting attempts surfaces a diagnostics screen
with the sidecar log tail.

**The mobile constraint this is built around:** iOS forbids spawning subprocesses.
`mf-net` is structured so that on a future iOS/Android port, `SimTransport` is
implemented by an in-process engine instead (an embedded JS runtime running the same
sim bundle, or a native port of the sim logic) with the trait's contract unchanged.
Every call site elsewhere in the workspace (`mf-state`'s event consumer, `mf-game`'s
state machine, every `mf-render` layer) only ever sees `Events<SimEvent>` and
`Res<SimLink>`; none of them know or care whether there's a subprocess underneath.
That is the entire reason this crate boundary exists where it does, rather than
folding transport concerns into `mf-game` directly.

### mf-state

Shared Bevy `Resource`s, filled from `mf-net`'s event stream by one system, and read
by both `mf-render` and `mf-game` without either depending on the other:

- `CurrentCity`: the `StaticCityJson` from `ready`, plus the 0-3 mask byte arrays
  that arrive right after it as binary `StaticMask` frames.
- `LatestFields`: the most recent `Fields` binary frame (terrain/population/jobs/
  land value/water/parks), sent at init and every 7 sim-days.
- `LatestFrame`: the most recent `FrameSnapshot` binary frame (vehicles, agents,
  color table), sent every 50 ms sim tick. No interpolation buffer in v1; only
  "latest" is retained.
- `LatestUi`: the most recent `UiState`, sent at 2 Hz (budget, approval, stations,
  tracks, routes, active events).
- `QualityTier`: the active quality tier and its knob table (see below).
- `SubwayView`: the subway-view toggle's target state and eased transition
  progress; `mf-game`'s input layer only flips the target, `mf-render`'s `subway.rs`
  is what actually steps the animation each frame (it's the module with per-frame
  delta time and the geometry to animate).
- `HeightAt`: a `Fn(x, z) -> y` ground-height sampler, defaulting to flat ground at
  `y = 0` until `mf-render`'s `terrain.rs` replaces it with a real bilinear sample
  once fields have loaded. Every layer that places something on the ground (roads,
  buildings, transit, vehicles, agents, the camera's ground raycast) depends on this
  resource rather than on `mf-render` directly, which is what lets it live in
  `mf-state` instead of creating a `mf-render` dependency for crates that shouldn't
  need one.

### mf-render

The 3D renderer, composed as `MfRenderPlugin` from one sub-plugin per visual layer,
ordered by a `MfRenderSet` system-set chain: `Terrain` (must run first and own any
`HeightAt` rebuild) then `Statics` (roads/buildings/transit: cache-checked every
frame against a version counter, only rebuilt on change) then `Dynamic`
(vehicles/agents/day-night/atmosphere/subway-view: run unconditionally every frame).

Buildings are the layer worth calling out specifically: 20 to 60 thousand static
cuboids per city are baked into **merged per-chunk meshes** (an 8x8 grid of world
chunks, one mesh per chunk), not GPU instancing. The buildings never move, so a
merged mesh gets whole-chunk frustum culling and a single draw call per visible
chunk for free, with no custom render pipeline required in Bevy. Instancing would
buy nothing here since there's no per-instance state to vary at draw time beyond
what's already baked into per-vertex color.

Vehicles are the inverse case: at most a few hundred at once, each one an entity with
its own `Transform`, so a grow-only entity pool with zero per-frame heap allocation
in steady state is simpler and just as fast as instancing would be at this count.

Every color in the renderer comes from `palette.rs`, the single source of truth for
the Mirror's Edge art direction (see `art-direction.md` at the repo root: it is
binding and overrides any conflicting guidance elsewhere). Full quality-tier knob
table: see the README, or `mf-state/src/quality.rs` for the source of truth.

### mf-game

The game shell (binary `metroforge`): the app state machine
(`Boot -> ConnectingSim -> MainMenu -> Loading -> InGame`, plus `SimError` after
exhausted sidecar reconnects), the RTS camera rig, input-to-command translation,
the egui HUD, and persistent config
(`config.toml` under the OS config directory, holding a quality-tier override that
always wins over auto-detection). This is the only crate that maps `mf-net`'s
`NetStatus` and `mf-state`'s readiness resources onto a concrete state machine;
neither of those crates knows these states exist.

## Determinism guarantee and the smoke-test gate

The sim core is deterministic: `(seed, command stream)` fully determines a game,
independent of wall-clock timing, frame rate, or which client issued the commands.
This client never simulates anything itself; it only renders what the sidecar sends
and forwards player input as commands. That means the determinism guarantee the sim
core already carries (golden replay tests in the `metroforge` repo) extends to this
client automatically, with one added risk: does the sidecar, once compiled with
`bun build --compile` into a single-file executable, still behave identically to the
interpreted `bun run` version, especially given that all city data is statically
embedded at compile time (see [`PROTOCOL.md`](PROTOCOL.md) and the sidecar README for
why dynamic imports don't survive `--compile`)?

That question is answered by the sidecar's own CI gate, not by anything in this
repo: `sidecar/smoke-test.ts` runs two independent sessions against the same seed,
builds an identical small bus network via commands, drives the sim forward several
hundred ticks, and asserts both that vehicles actually moved between consecutive
frames (the sim isn't stalled) and that `requestReplay`'s `stateHash` agrees between
the two runs at the same tick. It is run once interpreted and, in release CI, once
against a `bun build --compile` binary, so a divergence introduced by compilation
would be caught. `mf-net`'s `live_sidecar.rs` integration test is the complementary
check on this side of the boundary: it exercises the real Boot-to-Loading handshake
against a live sidecar process to confirm `mf-protocol`'s types actually decode real
sidecar output, not just the fixtures in `mf-protocol/tests/roundtrip.rs`. It is
`#[ignore]`d by default (the sidecar may not be built in every environment that runs
`cargo test`) and is meant to be run explicitly with
`cargo test -p mf-net --test live_sidecar -- --ignored`.

## Coordinate conventions

All crates agree on one convention: world X maps to Bevy X, world Y maps to Bevy Z,
and up is +Y. Units are meters throughout, with no unit scaling anywhere in the
pipeline. The ground is the XZ plane; the origin is the city center. Every wire
payload's `(x, y)` pair (vehicle positions, agent positions, station/track points,
field grid cells) is a world `(x, y)` and gets placed at Bevy `(x, heightAt(x, y),
y)` plus whatever per-layer vertical offset that layer needs (roads at
`heightAt + 0.5`, route stripes at `heightAt + 0.6`, vehicles at `heightAt + 3`, and
so on).

## Quality-tier knobs

`mf-state::QualityTier::knobs()` is the single source of truth; `mf-game`'s
`config.rs` persists an override, `mf-render`'s per-layer plugins each read the
fields relevant to them. See the table in the top-level README for the full
potato/low/medium/high breakdown, or `crates/mf-state/src/quality.rs` for the exact
values and the auto-detect heuristic (GPU adapter name and device kind).
