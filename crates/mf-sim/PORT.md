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
| types.ts (GameState subset) | `state.rs` | P0/P1 | SKELETON (P1 expands) |
| sim.ts (`simTick`) | `lib.rs::sim_tick` | P0/P3 | STUB (P3 real systems) |
| geometry.ts | `geometry.rs` | P1 | TODO |
| constants.ts | `constants.rs` | P1 | TODO |
| commands.ts | `commands.rs` | P1 | TODO |
| save.ts | `save.rs` | P1 | TODO |
| replay.ts | `replay.rs` | P1 | TODO |
| types.ts (full) | `types.rs` / `state.rs` | P1 | TODO |
| newGame.ts | `new_game.rs` | P2 | TODO |
| fields.ts | `fields.rs` | P2 | TODO |
| city/* | `city/` | P2 | TODO |
| scenario/*, scenarioRules.ts | `scenario/` | P2/P3 | TODO |
| transit/* | `transit/` | P3 | TODO |
| economy.ts | `economy.rs` | P3 | TODO |
| ops/* | `ops/` | P3 | TODO |
| geology.ts, geologyCost.ts | `geology.rs` | P3 | TODO |
| weather.ts, weatherEffects.ts | `weather.rs` | P3 | TODO |
| timeOfDay.ts | `time_of_day.rs` | P3 | TODO |
| events.ts | `events.rs` | P3 | TODO |
| analytics.ts | `analytics.rs` | P3 | TODO |
| instance.ts | (folded into state) | P1 | TODO |

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
