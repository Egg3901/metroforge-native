# mf-sim port plan

Native Rust port of the deterministic TypeScript sim in `sim/src/core`
(~10k lines). Runs in-process to replace the Bun sidecar (required for 1.0.0).
This crate is `crates/mf-sim`: pure sim, std-only plus optional `serde`, no
bevy / no rendering / no I/O.

## Determinism contract: NEW RUST BASELINE

The Rust sim defines **fresh golden `state_hash` values**. We do NOT try to
match JavaScript f64 math bit-for-bit. Two validation gates:

1. **Internal determinism (P0, done):** same seed + same command stream run
   twice produces the identical Rust `state_hash`. See
   `tests/determinism.rs` (500 ticks x2, plus a divergence check on a second
   seed).
2. **Behavioral tolerance vs the TS reference (later phases):** compare
   Rust-vs-TS on aggregate metrics within a tolerance band, not bit-for-bit.
   Out of scope for P0 beyond this scaffolding.

Hash algorithm: **FNV-1a 64-bit** (offset basis `0xcbf29ce484222325`, prime
`0x100000001b3`). Chosen for being tiny, dependency-free, byte-exact across
platforms, and well specified. Not cryptographic; we only need a stable
determinism fingerprint. State structs feed fields into `StateHasher` in a
**fixed, append-only order** (`GameState::hash_into`).

Guardrails held: seeded RNG only, no wall-clock, no `HashMap` iteration in
hashed paths (use `BTreeMap` / sorted `Vec` when maps arrive in P1+).

## RNG parity result

`rng.rs` is a faithful port of `sim/src/core/rng.ts` (xoshiro128\*\* with
splitmix32 seeding). The TS math is pure wrapping-u32 integer arithmetic, so
the Rust port reproduces the JS stream **bit-for-bit** — free RNG parity even
under the rebaseline. Verified against captured TS output for seed `12345`:

| surface | first outputs (seed 12345) |
|---|---|
| `next_uint()` | 1093274547, 203003357, 3741353573, 3803725158, 4178738660, ... |
| `next_f64()` | 0.25454777479171753, 0.04726535081863403, 0.8711017370223999, ... |
| `int(1,6)` | 2, 1, 6, 6, 6, 2, 2, 6, 6, 6 |
| initial `state()` | [3283241497, 613117429, 2940958500, 516375437] |
| `state()` after 1 draw | [4194198625, 1215451336, 2049103677, 1634976210] |

These are asserted exactly in `rng.rs` unit tests.

## Module port order

- **P0 (this milestone):** crate scaffold, `rng`, `hash`, minimal `state`,
  `sim_tick` stub, determinism harness.
- **P1 primitives:** geometry, constants, commands, save/replay, full
  `GameState` shape + typed-array fields.
- **P2 worldgen:** newGame, fields, city bundle, districts/roads.
- **P3 systems:** transit (assignment/vehicles/traffic), economy, ops,
  geology (+ cost), weather (+ effects), events, analytics, scenario.

## Per-file checklist (`sim/src/core/*.ts` -> Rust module)

| TS source | target Rust module | phase | status |
|---|---|---|---|
| rng.ts | `rng.rs` | P0 | DONE (bit-for-bit parity) |
| (baseline hash, no TS origin) | `hash.rs` | P0 | DONE |
| types.ts (GameState subset) | `state.rs` | P0/P1 | SUPERSEDED (folded into `types.rs`; `state.rs` removed) |
| sim.ts (`simTick`) | `sim.rs::sim_tick` | P0/P3 | DONE (full orchestrator; `lib.rs` re-exports) |
| geometry.ts | `geometry.rs` | P1 | DONE (full port incl. SpatialHash + Noise2D) |
| constants.ts | `constants.rs` | P1 | DONE (values verbatim; `MODES` -> `modes(mode)`) |
| commands.ts | `commands.rs` | P1 | DONE for the data model + pure edits; build/create/depot STUBBED (P2/P3) |
| save.ts | `save.rs` | P1 | DONE (`state_hash` mirrors save.ts field set/order; serde save/load behind `serde` feature) |
| replay.ts | `replay.rs` | P1 | TODO (needs new_game + sim systems; deferred to P2/P3) |
| types.ts (full) | `types.rs` | P1 | DONE (full `GameState`, hashed/transient split) |
| newGame.ts | `new_game.rs` | P2 | DONE (procedural; weather/scenario/OSM stubbed to P3/P4) |
| fields.ts | `fields.rs` | P2 | DONE (grid + cell/index/sample helpers) |
| city/generator.ts | `city/generator.rs` | P2 | DONE (procedural path; OSM path stubbed) |
| city/streamlines.ts | `city/streamlines.rs` | P2 | DONE |
| city/tensor.ts | `city/tensor.rs` | P2 | DONE |
| city/presets.ts | `city/presets.rs` | P2 | DONE (all 11 presets verbatim) |
| city/names.ts | `city/names.rs` | P2 | DONE (all name banks + generators) |
| city/osmCity.ts, osmRegistry.ts | `city/` (OSM) | P2 | STUB (P4/P5 wires real city data) |
| scenario/*, scenarioRules.ts | `scenario/` | P2/P3 | DONE (evaluate + rules); data-driven catalog -> content lane (P4/P5) |
| transit/* | `transit/` | P3 | DONE (assignment, build, road graph, traffic, grade effects, route path) |
| economy.ts | (folded into `sim.rs`) | P3 | DONE (`route_operating_cost` + daily economy in the orchestrator) |
| ops/* | `ops/` | P3 | DONE (v0.9 operations, wired) |
| geology.ts, geologyCost.ts | `geology.rs` | P3 | DONE (tunnel pricing wired into `build::track_cost`) |
| weather.ts, weatherEffects.ts | `weather.rs` | P3 | DONE (wired into assignment/ops/movement/build) |
| timeOfDay.ts | `transit/time_of_day.rs` | P3 | DONE (CANONICAL; `ops/periods` re-exports) |
| events.ts | `events.rs` | P3 | DONE (`ActiveEvent` reconciled onto `GameState`) |
| analytics.ts | `analytics.rs` | P3 | DONE (accumulator threaded via `state.analytics`) |
| instance.ts | (folded into state) | P1 | DONE (`GameState::instance_id`, transient) |

## P1 result: GameState shape + the hashed field set

`types.rs` now holds the full `GameState`. The hashed/transient split is
explicit and auditable:

- **Transient region** (`// ==== TRANSIENT ====`, all `#[serde(skip)]`):
  `instance_id`, `weather`, `last_weather_event`, `traffic`, `unserved`,
  `analytics`, `osm_water_mask`, `osm_park_mask`, `osm_building_mask`,
  `osm_mask_res`, `osm_elevation`, `osm_elev_res`, `osm_labels`,
  `poi_anchors`. This mirrors `save.ts::serialize`'s destructured exclusion
  set exactly. (`bankrupt_days` IS serialized but is NOT hashed.)
- **Two RNG streams kept separate:** `rng_state` (primary) and
  `ops_rng_state` (ops-only), as distinct `RngState` fields.
- **`Record<number, T>` maps -> `BTreeMap<u32, T>`** in hashed/iterated paths
  (`district_demand_mult`, `ops_daily`, and per-route `frequency`).
- **Typed-array grids -> `Vec<f32>` / `Vec<u8>`** on `FieldGrid` (+ `Vec<i16>`
  for `osm_elevation`).

### Exact hashed-field set + order (mirrored from `save.ts::stateHash`, l.186)

Implemented in `save.rs::state_hash`. Each numeric field is rounded to
micro-units (`round(v*1000)`) then mixed with FNV-1a (our algorithm choice;
the SET and ORDER follow the TS source, the numeric values do not):

1. `tick`
2. `budget.cash`
3. `stats.population`
4. `stations.len()`
5. `tracks.len()`
6. `routes.len()`
7. per route: `daily_ridership`, `vehicle_count`, `on_time_pct` (unset => 1.0)
8. per vehicle: `along`
9. per fleet unit: `condition`, `status` (0 active / 1 maintenance / 2 broken)
10. `incidents.len()` (0 when the ops sub-state is absent)

Everything else — `seed`, both RNG streams, `roads`, `districts`, `fields`,
the transient region — is deliberately NOT hashed, matching the TS source. A
determinism test in `save.rs` proves: hash stable across two calls; a hashed
field (`cash`) changes it; a non-hashed change (`instance_id`, `weather`,
`bankrupt_days`, `rng_state`) leaves it unchanged.

### Enum alignment with `mf-protocol`

`TransitMode`, `TrackGrade`, `Difficulty`, `PoiKind` (`PoiAnchorKind`),
`FailReason`, and the `SimCommand` variants mirror the wire enums in
`mf-protocol` variant-for-variant (same names, same order) so a P4 bridge is a
trivial `match`. They are DUPLICATED rather than re-exported so `mf-sim` stays
std-only plus an OPTIONAL `serde` feature (mf-protocol pulls in mandatory
serde/serde_json/thiserror). `RoadClass` and `Period` had no P0 mf-protocol
enum (protocol carries `cls` as a raw string / `period` as a string); they are
defined here to the same variant set. The one representational difference:
entity ids are `u32` in `SimCommand` (sim-internal) vs `i64` on the wire.

### Deferred to P2/P3

- Command bodies needing worldgen/systems are stubbed (return `ok: false` +
  `TODO`, not `todo!()` panics): `BuildStation`, `BuildTrack`, `CreateRoute`,
  `BuildDepot`, and the vehicle-resync / headway-derive parts of `EditRoute`.
- P3-owned transient payload types are concrete-but-empty placeholders with
  `TODO`: `WeatherSnapshot`, `WeatherEvent`, `TrafficField`, `UnservedDesire`,
  `AnalyticsState`, `MapLabel`. `ActiveEvent` / `ScenarioDef` are minimal
  persisted placeholders (id + a couple fields) pending P3.
- `replay.rs` deferred: it composes `new_game` (P2) + full `sim_tick` (P3).
- `serialize` does not yet collapse polylines to points-only (P2/P4 concern);
  it round-trips losslessly regardless.

## Flags for P1 (TS state shape)

- `GameState` is large (~50 fields) and optional-heavy; many fields are
  explicitly **transient / not hashed** (weather, traffic, analytics, osm\*
  masks, instanceId, bankruptDays). Model the hashed/transient split
  explicitly (separate sub-struct or a skip convention) so the fixed hash
  field order stays auditable. Do not flatten everything into one struct and
  hash it wholesale.
- Field grids use typed arrays (`Float32Array` / `Uint8Array`); port to
  `Vec<f32>` / `Vec<u8>` and hash as byte slices.
- There are TWO RNG streams on state (`rngState` + `opsRngState`) kept
  separate so ops randomness cannot reorder other systems — preserve that
  separation in Rust.
- Several `Record<number, ...>` maps exist (`districtDemandMult`); use
  `BTreeMap` in hashed paths to keep iteration order deterministic.

## P2 result: worldgen + new-game

The full procedural worldgen pipeline is ported. `new_game()` assembles a
complete initial `GameState` (fields, roads, districts, aggregated pop/jobs,
seeded primary + ops RNG streams, ops sub-state) from a seed + preset.

### What worldgen produces
terrain (0..1) + water mask, optional meandering river / terminal lake, a CBD
biased toward water, 3..5 employment subcenters, population + jobs density
fields (gaussian decay off CBD/subcenters, sprawl-scaled), procedural parks
(noise pockets + signature blocks), a tensor field (grid patches + global grid
for rigid presets + CBD radial + shoreline boundaries + value noise), arterial
+ local road streamlines with water bridging and junction snapping, land value +
NIMBY fields, and 4x4-cell districts with seed-stable unique names.

### Determinism proof (gate 1 — internal)
`tests/worldgen.rs::generation_is_bit_identical_run_twice` runs `generate_city`
twice for 4 (seed, preset, difficulty) triples and asserts every field grid
(terrain/water/parks/population/jobs/land_value/nimby), all road polyline
points, cbd, and district names are byte-identical.
`new_game_state_hash_is_deterministic` asserts `state_hash()` matches across two
`new_game` runs. All randomness draws from the seeded `Rng` (correct stream);
maps are `BTreeMap`; no wall-clock (`instance_id` is transient, unhashed).

### Behavioral acceptance (gate 2 — structural, tolerance-based)
TS reference captured by running `sim/src/core/city/generator.ts` under `bun`.
Rust vs TS (idiomatic f64, NOT bit-parity) landed effectively on the numbers:

| seed / preset / diff | metric | TS ref | Rust | tolerance |
|---|---|---|---|---|
| 12345 generic normal | waterFrac / parkFrac | 0.0867 / 0.1574 | 0.0867 / 0.1574 | ±0.03 abs |
| 12345 generic normal | fieldPop / districts / roads | 133447 / 453 / 1051 | 133447 / 453 / 1051 | ±5% / ±10% / ±10% |
| 777 generic normal | fieldPop / districts / roads | 130486 / 490 / 1100 | 130486 / 490 / 1100 | within band |
| 12345 nyc normal | waterFrac / districts / roads | 0.0387 / 433 / 985 | 0.0387 / 433 / 985 | within band |
| 42 boston easy | fieldPop / districts / roads | 191455 / 454 / 1052 | 191455 / 454 / 1050 | within band (roads -2 = 0.2%) |
| 999 atlanta hard | waterFrac / fieldPop / roads | 0.0 / 90767 / 1137 | 0.0 / 90767 / 1137 | within band |

The only divergence observed is a couple of local streamline segments (~0.2%,
boston) from f64 rounding in the tracer — comfortably inside the ±10% road
band. Asserted in `tests/worldgen.rs::structural_acceptance_vs_ts_reference`.
(A throwaway `crates/mf-sim/examples/p2_metrics.rs` prints the same metrics for
manual A/B against the TS harness.)

### Procedural vs OSM coverage
The **procedural** path is fully ported. Real presets (nyc, boston, ...) still
generate procedurally (the `else` branch), which is what runs whenever no OSM
bundle is supplied — always, in P2. The **OSM real-city path** (baked
water/park/building masks, real elevation, real road network, map labels, POI
anchors from `osmCity.ts`) is NOT ported: `new_game` has no `osm` option and the
generator omits the `if (osm)` branch. Those transient fields
(`osm_*`, `poi_anchors`, `osm_labels`) stay `None`. P4/P5 wires real city data.

### Stubbed / deferred to P3
- `new_game` leaves `weather` / `last_weather_event` = `None`
  (TODO: `weatherAt` + `climateTable`, weather.ts).
- Scenario derivation from a `ScenarioDef` (`rulesFromScenario`) is not wired;
  `new_game` accepts explicit `ScenarioRules` only.
- `period_for_tick(0)` is hardcoded to `Night` (tick 0 = midnight); the full
  `timeOfDay.ts` / `ops/periods.ts` port lands in P3.
- `initOps`'s route-reconcile loop is a no-op at new-game (no routes yet); the
  live fleet sync + per-tick ops logic is P3.

## P3 + orchestrator result: DONE

The full per-tick orchestrator (`sim.rs::sim_tick`) is wired and matches
`sim/src/core/sim.ts::simTick`. All P3 lanes (transit, ops, environment) plus
the deferred economy/scenario-rules helpers are integrated in-process.

### Tick order implemented (verified against sim.ts:164)
1. guard (failed / scenario-won) -> `tick += 1`
2. `update_weather` once per game-hour (or first tick): `climate_table` +
   `weather_at`, emit begin/end weather toasts.
3. `move_vehicles`: per-tick positions; `diurnal_factor` occupancy; grade x
   weather speed compose multiplicatively (`weather_speed_mult`).
4. ops `step` on the `OPS_INTERVAL` grid (frequency -> in-service units,
   condition decay, breakdown/recover rolls on `ops_rng_state`).
5. demand assignment on `demand_dirty` or every `ASSIGNMENT_INTERVAL_TICKS`:
   `run_assignment` -> reliability->ridership keystone (`apply_reliability_demand`)
   -> route capacity/crowding/segment-loads/surface-exposure/grade-speed ->
   station rolling ridership -> `capture_assignment_analytics` -> coverage ->
   `compute_traffic`.
6. daily boundary (`% TICKS_PER_DAY`): `update_events` -> `run_daily_economy`
   (fares x `event_fare_mult`, opex incl. `ops_daily_opex`, maintenance,
   subsidy, interest) -> `ops_daily_close` (BEFORE approval, so on-time% is
   fresh) -> `update_approval` (coverage + share + events + reliability -
   crowd-drag) -> `check_unlocks` -> weekly `run_growth` -> `commit_analytics_day`
   -> `check_failure` (bankruptcy grace, approval floor, time limit).

### Type reconciliation (types.rs unfrozen for integration)
The P1 empty transient placeholders were replaced with the real lane types
(all TRANSIENT / unhashed, so the frozen hashed set is untouched):
`state.weather: Option<weather::WeatherSnapshot>`,
`last_weather_event: Option<weather::WeatherEvent>`,
`traffic: Option<transit::traffic::TrafficFieldOut>`,
`unserved: Option<Vec<transit::assignment::UnservedDesire>>`,
`analytics: Option<analytics::AnalyticsState>` (accumulator threaded across
ticks via `ensure/capture/commit`). `active_events: Vec<events::ActiveEvent>`
(`{ id, days_left }`) is persisted (serde added) but NOT hashed. `save.rs`
was NOT touched (no hashed field type changed); the P1 transient-vs-hash test
still passes (it now stores a real `WeatherSnapshot`).

### Dedup
- Time-of-day: canonical in `transit/time_of_day.rs`; `ops/periods.rs` now
  re-exports `hour_of_day` / `diurnal_demand` / `diurnal_factor` (so
  `MEAN_RUSH_EXCESS` and `grade_effects::mean_rush_excess` share one curve).
- `route_cycle_seconds`: canonical `(tracks, fields, route)` in
  `transit/build.rs`; the ops-lane duplicate + its private grade/speed helpers
  (`day_average_surface_slowdown`, `segment_day_average_speed_mps`) were deleted
  and re-pointed at `transit::build` / `transit::grade_effects`.

### apply_command wiring
`commands.rs` dispatch now calls the real handlers:
`transit::build::{build_station, build_track, create_route, edit_route,
demolish_station, demolish_track}` and `ops::depot::build_depot` (the P1
`ok:false` stubs are gone). `create_route`/`edit_route` are followed by
`ops::sync_fleet_for_route`. `build::track_cost` prices tunnels through
`geology_cost::underground_segment_cost` (strata + depth), and `build_track`
applies the station-deepening surcharge; `weather_build_cost_mult` reads live
weather. Assignment's `deps` shim now reads real `weather_effects` + `events`.

### Determinism proof (gate 1 - internal)
`tests/full_tick.rs`:
- `full_turn_is_deterministic_run_twice`: `new_game(12345)` x 1300 ticks and
  `new_game(777)` x 2500 ticks each produce an identical `state_hash` run twice.
  (Ad-hoc: seed 777 @2500t hash `16956529555318314583` == itself.)
- `different_seeds_diverge`: 12345 vs 54321 differ.
- `scripted_network_is_deterministic`: a built bus network (stations+track+route
  via `apply_command`) run a game-day hashes identically run twice.
- `scripted_network_moves_riders_and_cash`: nonzero ridership (~337/day) and a
  finite, in-band cash trajectory.
Two RNG streams stay separate and seeded (`rng_state` events/growth,
`ops_rng_state` breakdowns); no wall-clock; hashed/ordered maps are `BTreeMap`.

### Behavioral acceptance (gate 2 - structural, vs TS via `bun`)
`behavioral_acceptance_vs_ts_reference`, seed 12345 normal, 3 sim-days:

| metric | TS ref | Rust | tolerance | result |
|---|---|---|---|---|
| population | 131812 | 131812 | +/- 1% | exact |
| cash | 15117664 | 15117664 | +/- 1% | exact |

The plain full-run lands ON the TS numbers to displayed precision. A built
network A/B (seed 2024, one game-day) gives ridership TS 315.6 vs Rust 336.6
(+6.7%, inside a +/- 10% band), approval 56.3 == 56.3, day-net ~+43.3k both.
The small ridership gap is idiomatic-Rust float divergence in the transit
assignment path (the accepted NEW RUST BASELINE), not a wiring defect.

### sim.ts behaviors NOT fully reproduced (flagged)
- **Data-driven scenarios.** `evaluateScenarioDay` (mid-run scenario events +
  lose-condition trees) is NOT wired: `scenario/evaluate.rs` ports the win/lose
  evaluation building blocks + `build_scenario_state`, but the catalog /
  progression content and the `ScenarioDef` win/lose trees are owned by the
  content lane. `new_game` leaves `state.scenario = None`, so the tick's
  scenario branch is inert (scenario RULES - approval floor / max-day / lock
  modes / subsidy override - ARE honored in `check_failure` / economy).
  `TickEvents.won` / `TickFail::Condition` are defined but only a real
  `ScenarioDef` would set them.
- **OSM real-city path** stays stubbed (transient `osm_*` fields `None`,
  `MapLabel` still a placeholder) - P5.

## P4 status: in-process swap DONE (additive + flagged)

The native Rust sim now runs IN-PROCESS behind the `SimTransport` seam. The
cutover is complete: the embedded transport is the sole sim transport. The Bun
sidecar and its `WsTransport` have been removed, and there is no longer any
backend selection.

### Transport
- There is no `MF_SIM` env var and no `SimBackend`; both are gone. `mf-game`'s
  `boot_system` builds the transport unconditionally via
  `SimLink::connect_embedded()`, which starts the in-process Rust sim
  (`EmbeddedTransport`).

### Where the code lives (mf-sim stays bevy-free AND serde-free)
- **`crates/mf-net/src/host.rs`** — the `GameState -> wire` serializer + the
  command bridge. Placed in `mf-net` (the one crate that already depends on
  BOTH `mf_sim` and the `mf_protocol` DTOs) so `mf-sim` needs no new deps.
- **`crates/mf-net/src/embedded.rs`** — `EmbeddedTransport impl SimTransport`.
- The transport is built in `crates/mf-net/src/plugin.rs` via
  `SimLink::connect_embedded`.

### Serializer coverage (`host.rs`, port of `host/protocol.ts` + `uiExtras.ts`)
- **`UiState`**: budget (cash/loan/lastDay/netHistory/lifetime), stats
  (population/approval/transitShare/coverage/dailyTransitTrips), unlockedModes,
  stations, tracks, routes, activeEvents (names via `event_by_id`), districts
  (+ `growthDelta`), insights (`compute_insights`), fieldsVersion, bankrupt,
  failed, maxDay/eraLabel, commandCount, hourOfDay/demandFactor/fareboxRecovery,
  overcrowdedRoutes, weather (state/intensity/season/event), and the full v0.9
  ops block: `fleet` summary, `depots`, `incidents`, `servicePeriod(+Label)`.
- **`UiRoute`**: base fields + v0.4.2 extras (operatingCost/farebox/liveCrowding/
  avgEffectiveSpeed) + v0.9 ops (onTimePct/avgDelaySec/inServiceVehicles/
  peakUnitsRequired/per-period `frequency`).
- **`FrameSnapshot`** (binary msgType=1): vehicles stride-6
  `[id,x,y,heading,occupancy,routeColorIdx]` via `point_along`, plus the packed
  `color_table` (parsed from route color hex).
- **`ready`** (`StaticCityJson`) + **`Fields`** (binary msgType=2) are emitted
  on `init` so the client can render the map/grids.
- Command bridge: `command_to_sim` (wire `Command` -> `SimCommand`, `i64`->`u32`
  ids, period string -> `Period`), `command_result_to_wire` (`u32`->`i64`).
- `queryTrackCost` -> `mf_sim::transit::build::track_cost`; `strataProbe` ->
  `geology::column_at`; `requestReplay` -> a `ReplayPayload` (see TODOs).

### EmbeddedTransport lifecycle + tick cadence
- `connect()` spawns a `mf-net-embedded` thread (crossbeam channels), and
  immediately queues `hello` so the client handshake completes. No tokio, no
  child process, and nothing to respawn for an in-process sim.
- The worker owns the `GameState` and mirrors `sim.worker.ts`: a 20 Hz step
  timer (`accumulator += speed/20`, up to 400 ticks/step), a `FrameSnapshot`
  every step, `UiState` at 2 Hz, `Fields` re-emitted every 7 sim-days, toasts
  from `TickEvents`. Only the step CADENCE uses a wall clock; the sim math
  stays seeded/deterministic. `SetSpeed`/`Command`/`init` handled inline.

### host/protocol.ts pieces NOT reproduced yet (flagged)
- **Agents** (`FrameSnapshot.agents`): the pedestrian particle pool
  (`host/agents.ts`, resampled from `state.flows`) is a host-side cosmetic
  layer, not sim state. `agent_count = 0` for now. TODO(P5).
- **`cohortDemand`** UiState field: needs `transit/cohorts.ts` HUD helpers
  (`cohort_mix` / `hourly_demand_curve`) not yet ported. Emitted as `None`
  (additive; client ignores). TODO(P5).
- **Saves** (`requestSave` / `loadSave`): need `mf-sim`'s `serde` feature,
  which `mf-net` does not enable; surfaced as a warn toast. TODO(P5).
- **`traffic` / `demand` / `heatmap`** overlays and OSM masks/labels/anchors
  are not emitted (procedural cities have no masks; overlays are P5).
- `requestReplay` returns tick/hash/seed but an empty `command_log` (a
  `SimCommand -> wire::Command` reverse bridge is still TODO); `replay.rs`
  itself remains unwritten.

## Remaining for P4 / P5
- **P4 residual:** reverse command bridge (`SimCommand -> wire::Command`) to
  populate replay `command_log`; `replay.rs` (compose `new_game` + `sim_tick`
  over `command_log`); perf measurement of the embedded path.
- **P5 (OSM city data):** the sidecar / dist-sidecar / `WsTransport` have been
  deleted. Port the OSM real-city path
  (`osmCity.ts` / `osmRegistry.ts`): baked water/park/building masks, real
  elevation, real road network, `MapLabel`, POI anchors; fill the transient
  `osm_*` slots and the `MapLabel` placeholder; wire the data-driven scenario
  catalog/progression + `evaluateScenarioDay`.

## P5 status: OSM real-city path DONE (flagship cities render from real data)

The embedded sim now renders real cities (NYC, Boston, ...) from the
baked OSM bundles instead of falling back to procedural generation.

### What was ported
- **`osmCity.ts` -> `city/osm.rs`** (mf-sim, serde-free): the `OsmCityData`
  model + the mask/elevation decoders (`decode_b64_mask`, `decode_elevation`,
  `mask_at`, a std-only base64 decoder) + `apply_osm_terrain` — the `if (osm)`
  branch of `generateCity`: AREA-fraction majority vote of the ~19 m mask over
  each 125 m field cell, multi-source BFS distance-to-water, shore-faded fbm
  relief. `MapLabel` fleshed out (`{kind, name, x, y, angle, imp}`) +
  `MapLabelKind` enum.
- **`generate_city`** now takes `osm: Option<&OsmCityData>` and branches:
  real land/water/parks + the real road network (with OSM grade/bridge/tunnel
  tags) replace procgen; the shared population / land-value / district stages
  still run on top. OSM draws no road RNG (geometry is static input). Procedural
  path is byte-unchanged (guarded by `osm.is_none()`).
- **`new_game`** gains `NewGameOptions.osm`; populates the transient
  `osm_water_mask` / `osm_park_mask` / `osm_building_mask` / `osm_mask_res` /
  `osm_elevation` / `osm_elev_res` / `osm_labels` / `poi_anchors` slots.
- **`osmRegistry.ts` + `sidecar/cities.ts` -> `mf-net/src/cities.rs`** (serde
  lives in mf-net): JSON parse (camelCase DTOs) -> `mf_sim::OsmCityData`, the
  10-key registry, and `resolve_city(key)`.
- **Serializer (`host.rs`)**: `build_ready` now emits `mask_res` + `has*Mask`
  flags + `labels` + `poi_anchors`; new `build_masks` (msgType=4 water/park/
  building, sidecar order) + `build_elevation` (msgType=7). `embedded.rs` sends
  them right after `ready`, matching `simHost.ts::sendStatic`.

### Data delivery
`mf-sim` stays serde-free, so JSON parsing + the base64 payloads live in
`mf-net`. Embedding all ten bundles (+ buildings) is ~90 MB, so:
- **NYC + Boston are embedded** via `include_bytes!` (~2 MB) — single-binary,
  always available, incl. tests.
- **The other eight load from a data dir** at runtime: `$MF_CITY_DATA` if set,
  else the compile-time in-repo `sim/src/data/cities`. Missing file -> `None` ->
  procedural fallback (never a hard failure).

### Acceptance (counts vs TS `osmCity`, nyc.json — EXACT)
`cargo test -p mf-net --test osm_city` (also asserts run-twice identical):

| channel | Rust | TS decode | note |
|---|---|---|---|
| mask res | 640 | 640 | |
| water mask set-cells | 73838 | 73838 | exact (pure decode) |
| park mask set-cells | 28636 | 28636 | exact |
| building mask set-cells | 93046 | 93046 | exact |
| elevation | 256^2, non-trivial | elevRes 256 | |
| roads | 13871 | 13871 (>=4 pts) | |
| labels | 513 | 513 | |
| POI anchors | 40 | 40 | all 5 kinds valid |

### In-client verify (`MF_AUTOSTART=nyc`, lavapipe/xvfb)
Looked at `verify/default.png`: minimap shows the unmistakable **Manhattan
silhouette** — the island flanked by the Hudson + East River (blue water
masks), the real street network, and the surrounding boroughs; the 3D view is a
dense field of real OSM building footprints (renderer log:
`buildings: mask res=640 set_cells=93046 lots_emitted=32050`,
`has_building_mask=true non_empty_chunks=64/64`). Real bridges were placed from
OSM tags (Brooklyn / Suspension / Truss), and station builds were correctly
rejected on real water ("Cannot build a station on water"). This is the REAL
city, not a procedural blob.

### Cities that load end-to-end
NYC + Boston embedded (verified). The other 8 keys resolve from the data dir
when present (same code path); NYC exercised fully in-client.

### Flagged / not reproduced
- **Buildings (msgType=5)**: the per-building footprint vectors
  (`*.buildings.json`, ~75 MB) are NOT emitted by the embedded host yet; the
  renderer derives massing from the building MASK instead (which it did here,
  32050 lots). Wiring `StaticBuildings` (embed cleveland+nyc, path-load rest)
  is a follow-up.
- **Road name/wikidata meta** (`staticCityWire.ts::roadMetaFromOsm`): bridge
  name labels are emitted as `None` (the bundles carry little/no road `name`).
- Embedded is now the sole transport (see cutover below).

### Historical remaining P5 items (resolved below)
- The sidecar / dist-sidecar / `WsTransport` have been deleted; embedded is the
  sole transport.
- `StaticBuildings` real-footprint emission (see above).
- Scenario catalog / progression + `evaluateScenarioDay`; `replay.rs` + reverse
  command bridge; agents pool (`FrameSnapshot.agents`); saves (`serde` feature);
  `cohortDemand`; traffic/demand/heatmap overlays; perf pass.

## Moonshot 1.0 cutover update

The Moonshot consolidation checklist is now complete for the shipped Rust path:

- Saves are wired end-to-end in embedded transport (`requestSave`/`loadSave`)
  using `mf-sim` serde, with replay-hash preservation tests.
- Host transient parity additions landed:
  - `UiState.cohortDemand` is populated from Rust cohort helpers.
  - Embedded transport now emits demand (`FromSimJson::Demand`) and traffic
    (`msgType=3`) on assignment refresh.
  - `FrameSnapshot.agents` is driven by a host-side particle pool resampled from
    active transit flows.
- CUTOVER is applied:
  - Embedded sim is the runtime default.
  - Sidecar runtime/compile paths are removed from packaging and CI workflows.
  - `sim/sidecar/` source tree is removed.

Decision note: the old sidecar-only heatmap transport frame has no live native
consumer path in this tree; parity work focused on the overlays actively used by
`mf-state`/`mf-game` (`demand` and `traffic`) plus frame/UI transients.
