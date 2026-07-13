//! `mf-sim` — native Rust port of the deterministic MetroForge simulation core
//! (`sim/src/core`). Pure sim: no bevy, no rendering, no I/O, std-only (plus
//! optional serde). Intended to run in-process, replacing the Bun sidecar.
//!
//! Determinism contract (NEW RUST BASELINE): the Rust sim defines fresh golden
//! `state_hash` values and does NOT match JavaScript f64 math bit-for-bit. The
//! RNG (see [`rng`]) is the one exception and reproduces the TS output exactly,
//! giving us free RNG parity. Validation in P0 is internal determinism only:
//! same seed + same commands twice -> identical hash. See `PORT.md`.
//!
//! Guardrails: seeded RNG only, no wall-clock, no HashMap iteration in hashed
//! paths. See the individual modules for their TS source mapping.

pub mod hash;
pub mod rng;
pub mod state;

pub use hash::{Hashable, StateHasher};
pub use rng::{Rng, RngState};
pub use state::GameState;

/// Advance the simulation by one tick. Mirrors the TS entry `simTick`
/// (sim/src/core/sim.ts:164).
///
/// P0 PLACEHOLDER LOGIC: increments the tick, draws one value from the seeded
/// RNG, and folds it deterministically into the scalar `cash` field. The real
/// per-tick systems (weather, vehicle movement, ops, demand assignment, daily
/// economy, approval, scenario evaluation) land in P3. The point of this stub
/// is purely to exercise the deterministic tick + RNG + hash pipeline.
pub fn sim_tick(state: &mut GameState) {
    state.tick += 1;
    // Draw from the seeded stream so RNG state advances every tick.
    let roll = state.rng.next_f64();
    // Deterministic placeholder economy mutation (real economy is P3).
    state.cash += roll - 0.5;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_advances_and_is_deterministic() {
        let mut a = GameState::new(42);
        let mut b = GameState::new(42);
        for _ in 0..100 {
            sim_tick(&mut a);
            sim_tick(&mut b);
        }
        assert_eq!(a.tick, 100);
        assert_eq!(a.state_hash(), b.state_hash());
    }
}
