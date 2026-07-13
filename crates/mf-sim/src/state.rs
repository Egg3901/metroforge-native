//! Minimal `GameState` skeleton. Maps to `sim/src/core/types.ts` `GameState`.
//!
//! P0 SCOPE: this holds ONLY the fields needed to prove a deterministic tick
//! loop end to end (tick counter, embedded RNG state, seed, and one scalar
//! economy field). It is deliberately tiny.
//!
//! GROWTH POINT:
//! // P1 expands this to mirror sim/src/core/types.ts GameState
//! P1 will add the primitive scalars (difficulty, cityKey, nextId,
//! demandDirty, failed, bankruptDays, budget/stats scalars), P2 the worldgen
//! collections (fields, districts, roads), P3 the transit/economy/ops/geology
//! /weather subsystems. See PORT.md for the full order and per-file status.
//!
//! NOTE for P1 (state-shape flags): the TS `GameState` is a large, mostly
//! optional-heavy interface (~50 fields, many `?` + transient). Two things to
//! plan for:
//!   * Transient vs hashed: several fields (weather, traffic, analytics, osm*
//!     masks, instanceId, bankruptDays) are explicitly NOT part of the state
//!     hash. Rust should model this with a clear split (e.g. a separate
//!     `Transient` sub-struct or a `#[hash(skip)]` convention) rather than one
//!     flat struct, so the hasher field order stays auditable.
//!   * Typed arrays: TS uses Float32Array/Uint8Array for the field grids;
//!     Rust should use `Vec<f32>` / `Vec<u8>` and hash them as byte slices.

use crate::hash::{Hashable, StateHasher};
use crate::rng::{Rng, RngState};

/// Minimal deterministic game state. See module docs for the P1+ growth plan.
#[derive(Clone, Debug)]
pub struct GameState {
    /// Original numeric seed the run was created from. Mirrors `seed`.
    pub seed: u32,
    /// Monotonic tick counter (1 tick = 1 game-second). Mirrors `tick`.
    pub tick: u64,
    /// Saved RNG stream state. Mirrors `rngState`.
    pub rng: Rng,
    /// Placeholder scalar economy field. Stands in for `budget.cash` until P3
    /// ports the real economy. Mirrors (loosely) `budget.cash`.
    pub cash: f64,
}

impl GameState {
    /// Create a fresh state from a seed. Mirrors `newGame` seeding just enough
    /// to drive the tick loop; the real world/economy init lands in P2/P3.
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            tick: 0,
            rng: Rng::from_seed(seed),
            cash: 1_000_000.0,
        }
    }

    /// Restore a state from its persisted primitive parts (save/load skeleton).
    pub fn from_parts(seed: u32, tick: u64, rng_state: RngState, cash: f64) -> Self {
        Self {
            seed,
            tick,
            rng: Rng::from_state(rng_state),
            cash,
        }
    }

    /// Fingerprint the deterministic (hashed) fields in a FIXED order. This is
    /// the single source of truth for the P0 state hash. New hashed fields are
    /// appended here in P1+ (never reordered) so golden hashes stay stable.
    pub fn state_hash(&self) -> u64 {
        let mut h = StateHasher::new();
        self.hash_into(&mut h);
        h.finish()
    }
}

impl Hashable for GameState {
    fn hash_into(&self, h: &mut StateHasher) {
        // FIXED ORDER. Append-only across phases.
        h.write_u32(self.seed);
        h.write_u64(self.tick);
        let s = self.rng.state();
        h.write_u32(s[0]);
        h.write_u32(s[1]);
        h.write_u32(s[2]);
        h.write_u32(s[3]);
        h.write_f64(self.cash);
    }
}
