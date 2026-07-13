//! Determinism integration test for the P0 sim foundation.
//!
//! Proves the two P0 validation requirements:
//!   1. same seed + same command stream twice -> identical state hash;
//!   2. two different seeds diverge.

use mf_sim::{sim_tick, GameState};

const TICKS: usize = 500;

fn run(seed: u32) -> u64 {
    let mut state = GameState::new(seed);
    for _ in 0..TICKS {
        sim_tick(&mut state);
    }
    assert_eq!(state.tick, TICKS as u64);
    state.state_hash()
}

#[test]
fn same_seed_is_bit_identical() {
    assert_eq!(run(12345), run(12345));
}

#[test]
fn different_seeds_diverge() {
    assert_ne!(run(12345), run(54321));
}
