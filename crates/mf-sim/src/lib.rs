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

pub mod analytics;
pub mod city;
pub mod commands;
pub mod constants;
pub mod events;
pub mod fields;
pub mod geology;
pub mod geology_cost;
pub mod geometry;
pub mod hash;
pub mod new_game;
pub mod ops; // P3-OPS ADDED: register the v0.9 operations module (lane B).
pub mod replay;
pub mod rng;
pub mod save;
pub mod scenario;
pub mod sim;
pub mod transit;
pub mod types;
pub mod weather;
pub mod weather_effects;

pub use commands::{apply_command, CommandResult, SimCommand};
pub use hash::{Hashable, StateHasher};
pub use new_game::{new_game, NewGameOptions};
pub use rng::{Rng, RngState};
pub use save::state_hash;
pub use sim::{sim_tick, TickEvents};
pub use types::GameState;

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
