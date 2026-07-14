//! P2 worldgen validation.
//!
//! Two gates (see `PORT.md`):
//! 1. **Internal determinism** — same seed + preset produces byte-identical
//!    field grids, road/district counts, AND `state_hash`, run twice.
//! 2. **Behavioral acceptance** — the generated city is structurally similar to
//!    the TS reference (captured via `sim/tsref_p2.ts`), within documented
//!    tolerance bands. NOT bit-identical vs TS (the new Rust baseline).

use mf_sim::city::{generate_city, preset_by_key, GeneratedCity};
use mf_sim::new_game::{new_game, NewGameOptions};
use mf_sim::types::{Difficulty, FieldGrid, RoadClass};

fn gen(seed: u32, key: &str, diff: Difficulty) -> GeneratedCity {
    generate_city(seed, diff, None, preset_by_key(Some(key)), None)
}

fn fields_eq(a: &FieldGrid, b: &FieldGrid) -> bool {
    a.w == b.w
        && a.h == b.h
        && a.terrain == b.terrain
        && a.water == b.water
        && a.parks == b.parks
        && a.population == b.population
        && a.jobs == b.jobs
        && a.land_value == b.land_value
        && a.nimby == b.nimby
}

#[test]
fn generation_is_bit_identical_run_twice() {
    for (seed, key, diff) in [
        (12345u32, "generic", Difficulty::Normal),
        (12345, "nyc", Difficulty::Normal),
        (42, "boston", Difficulty::Easy),
        (999, "atlanta", Difficulty::Hard),
    ] {
        let a = gen(seed, key, diff);
        let b = gen(seed, key, diff);
        assert!(
            fields_eq(&a.fields, &b.fields),
            "fields differ {key}/{seed}"
        );
        assert_eq!(a.roads.len(), b.roads.len(), "road count {key}/{seed}");
        assert_eq!(a.districts.len(), b.districts.len(), "district count");
        assert_eq!(a.cbd, b.cbd, "cbd {key}/{seed}");
        // full road geometry + district names identical
        for (ra, rb) in a.roads.iter().zip(&b.roads) {
            assert_eq!(ra.polyline.points, rb.polyline.points);
        }
        let na: Vec<_> = a.districts.iter().map(|d| &d.name).collect();
        let nb: Vec<_> = b.districts.iter().map(|d| &d.name).collect();
        assert_eq!(na, nb, "district names {key}/{seed}");
    }
}

#[test]
fn new_game_state_hash_is_deterministic() {
    let opts = || NewGameOptions {
        preset_key: Some("nyc".into()),
        ..Default::default()
    };
    let a = new_game(12345, Difficulty::Normal, opts());
    let b = new_game(12345, Difficulty::Normal, opts());
    assert_eq!(a.state_hash(), b.state_hash());
    assert_eq!(a.stats.population, b.stats.population);
    assert_eq!(a.roads.len(), b.roads.len());
    assert!(a.stats.population > 0.0);
    assert!(a.ops_rng_state.is_some());
}

/// TS reference metrics captured from `sim/tsref_p2.ts` (bun, MetroForge sim
/// v1.1.0). Tolerance bands are generous because the Rust baseline is idiomatic
/// f64, NOT JS bit-parity. In practice the RNG parity + faithful port land the
/// Rust output on the TS numbers exactly for fields/pop/districts; only a
/// couple of streamline road segments can differ (~0.2%) from f64 rounding.
struct Ref {
    seed: u32,
    key: &'static str,
    diff: Difficulty,
    water_frac: f64,
    park_frac: f64,
    field_pop: f64,
    districts: usize,
    roads: usize,
}

#[test]
fn structural_acceptance_vs_ts_reference() {
    let refs = [
        Ref {
            seed: 12345,
            key: "generic",
            diff: Difficulty::Normal,
            water_frac: 0.0867,
            park_frac: 0.1574,
            field_pop: 133447.0,
            districts: 453,
            roads: 1051,
        },
        Ref {
            seed: 777,
            key: "generic",
            diff: Difficulty::Normal,
            water_frac: 0.0792,
            park_frac: 0.1496,
            field_pop: 130486.0,
            districts: 490,
            roads: 1100,
        },
        Ref {
            seed: 12345,
            key: "nyc",
            diff: Difficulty::Normal,
            water_frac: 0.0387,
            park_frac: 0.1651,
            field_pop: 132777.0,
            districts: 433,
            roads: 985,
        },
        Ref {
            seed: 42,
            key: "boston",
            diff: Difficulty::Easy,
            water_frac: 0.1273,
            park_frac: 0.1292,
            field_pop: 191455.0,
            districts: 454,
            roads: 1052,
        },
        Ref {
            seed: 999,
            key: "atlanta",
            diff: Difficulty::Hard,
            water_frac: 0.0000,
            park_frac: 0.1452,
            field_pop: 90767.0,
            districts: 531,
            roads: 1137,
        },
    ];

    for r in refs {
        let c = gen(r.seed, r.key, r.diff);
        let f = &c.fields;
        let n = (f.w * f.h) as f64;
        let water = f.water.iter().filter(|&&v| v == 1).count() as f64 / n;
        let parks = f.parks.iter().filter(|&&v| v == 1).count() as f64 / n;
        let pop: f64 = f.population.iter().map(|&v| v as f64).sum();

        // dimensions: exact
        assert_eq!(f.w, 96, "{}", r.key);
        assert_eq!(f.h, 96, "{}", r.key);

        // coverage fractions: within 0.03 absolute
        assert!(
            (water - r.water_frac).abs() < 0.03,
            "{} water {water} vs {}",
            r.key,
            r.water_frac
        );
        assert!(
            (parks - r.park_frac).abs() < 0.03,
            "{} parks {parks} vs {}",
            r.key,
            r.park_frac
        );

        // population: within 5%
        assert!(
            (pop - r.field_pop).abs() / r.field_pop < 0.05,
            "{} pop {pop} vs {}",
            r.key,
            r.field_pop
        );

        // district count: within 10%
        let dd = (c.districts.len() as f64 - r.districts as f64).abs() / r.districts as f64;
        assert!(
            dd < 0.10,
            "{} districts {} vs {}",
            r.key,
            c.districts.len(),
            r.districts
        );

        // road count: within 10%
        let rd = (c.roads.len() as f64 - r.roads as f64).abs() / r.roads as f64;
        assert!(
            rd < 0.10,
            "{} roads {} vs {}",
            r.key,
            c.roads.len(),
            r.roads
        );

        // sanity: some of each road class + all districts named
        assert!(c.roads.iter().any(|e| e.cls == RoadClass::Arterial));
        assert!(c.roads.iter().any(|e| e.cls == RoadClass::Local));
        assert!(c.districts.iter().all(|d| !d.name.is_empty()));
    }
}
