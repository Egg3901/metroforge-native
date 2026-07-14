//! Full-tick integration tests for the P3 orchestrator (`sim_tick`).
//!
//! Proves the determinism contract on the REAL orchestrator wired end to end:
//!   (a) new_game(seed) run 1200+ ticks -> identical state_hash run twice;
//!   (b) two different seeds diverge;
//!   (c) a scripted network produces nonzero ridership + a plausible cash
//!       trajectory over a game-day;
//!   (d) behavioral acceptance vs a TS full-run reference within a documented
//!       tolerance band (see `full_run_reference` / the tolerance notes below).
//!
//! The baseline is the NEW RUST BASELINE (idiomatic Rust, not JS f64 bit
//! parity); the two RNG streams stay separate and seeded.

use mf_sim::commands::SimCommand;
use mf_sim::types::{Difficulty, TrackGrade, TransitMode};
use mf_sim::{apply_command, new_game, sim_tick, GameState, NewGameOptions};

const TICKS_PER_DAY: u64 = 1200;

/// Run a fresh procedural game (no built network) for `ticks` and return the
/// hash. Exercises the full tick loop (weather, ops, daily economy, events,
/// growth, analytics) on the generated city.
fn run_plain(seed: u32, ticks: u64) -> u64 {
    let mut s = new_game(seed, Difficulty::Normal, NewGameOptions::default());
    for _ in 0..ticks {
        sim_tick(&mut s);
    }
    s.state_hash()
}

/// Build a small bus network on a fresh game: pick a few district centroids as
/// station sites (they sit on land near roads, so bus snapping succeeds), lay
/// track between them, and create a route. Returns the state ready to run.
fn scripted_network(seed: u32) -> GameState {
    let mut s = new_game(seed, Difficulty::Normal, NewGameOptions::default());
    // Anchor on the highest-population district (the CBD) and pick its nearest
    // neighbors, so the line is compact and sits in real demand (a route strung
    // across the whole map gets max headway and no riders).
    let anchor = s
        .districts
        .iter()
        .max_by(|a, b| a.population.partial_cmp(&b.population).unwrap())
        .map(|d| d.centroid)
        .unwrap();
    let mut near: Vec<mf_sim::geometry::Vec2> = s.districts.iter().map(|d| d.centroid).collect();
    near.sort_by(|a, b| {
        let da = (a.x - anchor.x).hypot(a.y - anchor.y);
        let db = (b.x - anchor.x).hypot(b.y - anchor.y);
        da.partial_cmp(&db).unwrap()
    });
    let picks: Vec<mf_sim::geometry::Vec2> = near.into_iter().take(4).collect();

    let mut station_ids: Vec<u32> = Vec::new();
    for p in &picks {
        let r = apply_command(
            &mut s,
            &SimCommand::BuildStation {
                mode: TransitMode::Bus,
                pos: *p,
            },
        );
        if let Some(id) = r.created_id {
            station_ids.push(id);
        }
    }
    // lay surface track between consecutive built stations.
    let mut linked: Vec<u32> = Vec::new();
    for w in station_ids.windows(2) {
        let r = apply_command(
            &mut s,
            &SimCommand::BuildTrack {
                mode: TransitMode::Bus,
                grade: TrackGrade::Surface,
                from_station_id: w[0],
                to_station_id: w[1],
                waypoints: Vec::new(),
            },
        );
        if r.ok {
            if linked.last() != Some(&w[0]) {
                linked.push(w[0]);
            }
            linked.push(w[1]);
        }
    }
    if linked.len() >= 2 {
        apply_command(
            &mut s,
            &SimCommand::CreateRoute {
                mode: TransitMode::Bus,
                station_ids: linked,
            },
        );
    }
    s
}

// (a) full-turn determinism: identical hash run twice, 1200+ ticks.
#[test]
fn full_turn_is_deterministic_run_twice() {
    assert_eq!(run_plain(12345, 1300), run_plain(12345, 1300));
    assert_eq!(run_plain(777, 2500), run_plain(777, 2500));
}

// (b) two different seeds diverge over a full turn.
#[test]
fn different_seeds_diverge() {
    assert_ne!(run_plain(12345, 1300), run_plain(54321, 1300));
}

// scripted-network determinism (also run twice) with commands applied.
#[test]
fn scripted_network_is_deterministic() {
    let run = |seed| {
        let mut s = scripted_network(seed);
        for _ in 0..TICKS_PER_DAY {
            sim_tick(&mut s);
        }
        s.state_hash()
    };
    assert_eq!(run(2024), run(2024));
}

// (c) a scripted network produces nonzero ridership + a plausible cash path.
#[test]
fn scripted_network_moves_riders_and_cash() {
    let mut s = scripted_network(2024);
    assert!(
        s.stations.len() >= 2 && !s.routes.is_empty(),
        "network did not build: {} stations, {} routes",
        s.stations.len(),
        s.routes.len()
    );
    let cash0 = s.budget.cash;
    // run a full game-day so a daily economy close happens.
    for _ in 0..TICKS_PER_DAY {
        sim_tick(&mut s);
    }
    let ridership: f64 = s.routes.iter().map(|r| r.daily_ridership).sum();
    assert!(
        ridership > 0.0,
        "expected nonzero ridership, got {ridership}"
    );
    // cash moved (fares - opex - maintenance + subsidy on the day close) and
    // stays in a plausible band (not NaN, not wild).
    assert!(s.budget.cash.is_finite());
    assert!(
        (s.budget.cash - cash0).abs() > 0.0,
        "cash should move over a game-day"
    );
    assert!(
        s.budget.cash > -5_000_000.0 && s.budget.cash < cash0 + 5_000_000.0,
        "cash out of plausible band: {} -> {}",
        cash0,
        s.budget.cash
    );
}

// (d) behavioral acceptance vs a TS full-run reference.
//
// TOLERANCE BAND (documented): the Rust sim is the NEW RUST BASELINE and is NOT
// bit-identical to the TS reference. We assert STRUCTURAL agreement on the
// headline aggregates at a fixed checkpoint for a fresh procedural game (no
// built network, so the numbers are driven purely by worldgen + the daily
// economy / growth loop, which is what the TS harness reports too):
//
// The TS reference numbers below were captured by running `sim/src/core`
// (`newGame` + `simTick`) under `bun` for the SAME seed/config (see PORT.md).
// The Rust full-run lands ON the TS numbers to displayed precision:
//
//   seed 12345, generic preset, normal, checkpoint = 3 sim-days (3600 ticks)
//     metric      TS ref      Rust        tolerance asserted
//     population  131812      131812      +/- 1%  (exact match)
//     cash        15117664    15117664    +/- 1%  (exact match)
//
// (A built-network A/B, seed 2024 over one game-day, gives ridership
// TS 315.6 vs Rust 336.6 = +6.7%, approval 56.3 == 56.3, day-net ~+43.3k both;
// documented in PORT.md. The transit-assignment float paths diverge slightly
// under the Rust baseline, comfortably inside a +/- 10% ridership band.)
#[test]
fn behavioral_acceptance_vs_ts_reference() {
    let mut s = new_game(12345, Difficulty::Normal, NewGameOptions::default());
    let pop0 = s.stats.population;
    for _ in 0..(3 * TICKS_PER_DAY) {
        sim_tick(&mut s);
    }
    // population matches the TS reference within 1%.
    let ts_pop = 131_812.0;
    assert!(
        (s.stats.population - ts_pop).abs() / ts_pop < 0.01,
        "population {} not within 1% of TS ref {ts_pop}",
        s.stats.population
    );
    // a 3-day no-network run barely moves population (access==0 cells decay 0.05%/day).
    assert!(
        (s.stats.population - pop0).abs() / pop0 < 0.05,
        "population drifted too far over 3 days: {pop0} -> {}",
        s.stats.population
    );
    // cash matches the TS reference within 1%.
    let ts_cash = 15_117_664.0;
    assert!(s.budget.cash.is_finite());
    assert!(
        (s.budget.cash - ts_cash).abs() / ts_cash < 0.01,
        "cash {} not within 1% of TS ref {ts_cash}",
        s.budget.cash
    );
}
