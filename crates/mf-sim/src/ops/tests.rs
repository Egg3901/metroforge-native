//! Tests for the v0.9 operations lane: run-twice determinism, the reliability
//! keystone (on-time% + delay feeding approval AND ridership), and behavioral
//! tolerance vs the TS `ops/index.ts` formulas.

use super::*;
use crate::geometry::{make_polyline, vec};
use crate::save::state_hash;
use crate::types::{Difficulty, GameState, RouteDef, TrackSegment};

/// Build a straight track segment `len` meters long on the given grade.
fn track(
    id: u32,
    mode: TransitMode,
    grade: TrackGrade,
    from: u32,
    to: u32,
    len: f64,
) -> TrackSegment {
    TrackSegment {
        id,
        mode,
        grade,
        from_station_id: from,
        to_station_id: to,
        polyline: make_polyline(vec![vec(0.0, id as f64 * 10.0), vec(len, id as f64 * 10.0)]),
        build_cost: 0.0,
        congestion_density: None,
    }
}

/// Build a route over the given stations/segments with `n` vehicles.
fn route(id: u32, mode: TransitMode, stations: Vec<u32>, segments: Vec<u32>, n: u32) -> RouteDef {
    RouteDef {
        id,
        name: format!("Route {id}"),
        color: "#fff".to_string(),
        mode,
        station_ids: stations,
        segment_ids: segments,
        headway_seconds: 300.0,
        fare: 2.5,
        vehicle_count: n,
        daily_ridership: 0.0,
        daily_revenue: 0.0,
        capacity: 0.0,
        load: 0.0,
        crowding: 0.0,
        segment_loads: Vec::new(),
        surface_exposure: None,
        move_grade_speed: None,
        frequency: None,
        scheduled_headway: None,
        in_service_vehicles: None,
        on_time_pct: None,
        avg_delay_sec: None,
        reliability_demand_mult: None,
    }
}

/// A hand-built network: two bus routes with fleet, ready for the ops tick.
fn build_network(seed: u32, difficulty: Difficulty) -> GameState {
    let mut s = GameState::with_difficulty(seed, difficulty);
    s.fields = crate::fields::create_field_grid(None);
    s.tracks = vec![
        track(1, TransitMode::Bus, TrackGrade::Surface, 10, 11, 2000.0),
        track(2, TransitMode::Bus, TrackGrade::Surface, 11, 12, 2000.0),
        track(3, TransitMode::Metro, TrackGrade::Tunnel, 20, 21, 3000.0),
    ];
    s.routes = vec![
        route(100, TransitMode::Bus, vec![10, 11, 12], vec![1, 2], 4),
        route(200, TransitMode::Metro, vec![20, 21], vec![3], 3),
    ];
    // moderate crowding on the bus line raises its breakdown risk.
    s.routes[0].crowding = 1.4;
    init_ops(&mut s);
    s
}

#[test]
fn init_ops_mints_fleet_from_vehicle_count() {
    let s = build_network(12345, Difficulty::Normal);
    let fleet = s.fleet.as_ref().unwrap();
    assert_eq!(fleet.len(), 7, "4 bus + 3 metro units minted");
    assert!(fleet
        .iter()
        .all(|u| u.condition == 1.0 && u.status == FleetStatus::Active));
    // scheduled_headway frozen to the command-set headway.
    assert_eq!(s.routes[0].scheduled_headway, Some(300.0));
}

#[test]
fn ops_step_is_deterministic_run_twice() {
    // Same seed + same tick stream twice -> identical state hash and fleet.
    let mut a = build_network(777, Difficulty::Hard);
    let mut b = build_network(777, Difficulty::Hard);
    for _ in 0..400 {
        a.tick += OPS_INTERVAL as u64;
        b.tick += OPS_INTERVAL as u64;
        let ra = step(&mut a);
        let rb = step(&mut b);
        assert_eq!(ra, rb, "step results diverged at tick {}", a.tick);
        if a.tick % (OPS_INTERVAL as u64 * 60) == 0 {
            ops_daily_close(&mut a);
            ops_daily_close(&mut b);
        }
    }
    assert_eq!(state_hash(&a), state_hash(&b), "ops tick not deterministic");
    assert_eq!(a.fleet, b.fleet);
    assert_eq!(a.incidents, b.incidents);
}

#[test]
fn hard_mode_produces_breakdowns_and_recovers() {
    // Under HARD, worn + crowded stock breaks down; incidents open and later
    // clear (units limp back). This exercises the full breakdown -> block ->
    // recover cycle deterministically.
    let mut s = build_network(42, Difficulty::Hard);
    // pre-wear the bus fleet so condition risk is high.
    for u in s.fleet.as_mut().unwrap().iter_mut() {
        if u.route_id == Some(100) {
            u.condition = 0.2;
        }
    }
    let mut ever_broken = 0;
    for _ in 0..500 {
        s.tick += OPS_INTERVAL as u64;
        let r = step(&mut s);
        ever_broken += r.toasts.len();
    }
    assert!(
        ever_broken > 0,
        "hard mode with worn crowded stock should break down"
    );
    // after the run, incidents eventually clear (no permanent lock-up).
    let mut tail = 0;
    for _ in 0..200 {
        s.tick += OPS_INTERVAL as u64;
        step(&mut s);
        tail = s.incidents.as_ref().unwrap().len();
    }
    // a bounded number of concurrent incidents (one per route at most here).
    assert!(
        tail <= 2,
        "incidents should not accumulate unbounded, got {tail}"
    );
}

#[test]
fn condition_decay_matches_ts_formula() {
    // TS reference (ops/index.ts decayCondition, FORGIVING tunables), captured
    // analytically because the decay is pure f64 arithmetic with no RNG:
    //   metersPerInterval = speed(bus=8.3) * OPS_INTERVAL(20) = 166 m
    //   decay/interval     = conditionDecayPerMeter(1e-6) * 166 * 1.0 = 1.66e-4
    //   over 60 intervals (one 1200-tick day) = 9.96e-3 condition/day.
    let mut s = build_network(1, Difficulty::Normal);
    let unit_id = s
        .fleet
        .as_ref()
        .unwrap()
        .iter()
        .find(|u| u.route_id == Some(100))
        .unwrap()
        .id;
    for _ in 0..60 {
        s.tick += OPS_INTERVAL as u64;
        step(&mut s);
    }
    let cond = s
        .fleet
        .as_ref()
        .unwrap()
        .iter()
        .find(|u| u.id == unit_id)
        .unwrap()
        .condition;
    let ts_ref = 1.0 - 60.0 * (1e-6 * 8.3 * 20.0);
    // tolerance: 1e-4 absolute (well inside the ~0.01/day decay).
    assert!((cond - ts_ref).abs() < 1e-4, "cond={cond} ts_ref={ts_ref}");
}

#[test]
fn keystone_reliability_feeds_ridership_and_approval() {
    // The v0.9 keystone: on-time% + delay drive BOTH ridership demand AND
    // approval. Verify both directions off the same reliability figure.
    let mut s = build_network(5, Difficulty::Normal);
    let t = ops_tunables(Difficulty::Normal);

    // A reliable route (on-time at target) keeps all riders; a chronically late
    // one (50% on-time) sheds them.
    s.routes[0].on_time_pct = Some(1.0);
    s.routes[0].reliability_demand_mult = Some(reliability_demand_mult_for(1.0, &t));
    s.routes[1].on_time_pct = Some(0.5);
    s.routes[1].reliability_demand_mult = Some(reliability_demand_mult_for(0.5, &t));

    let mut ridership = BTreeMap::new();
    ridership.insert(100u32, 1000.0);
    ridership.insert(200u32, 1000.0);
    let mut revenue = BTreeMap::new();
    revenue.insert(100u32, 500.0);
    revenue.insert(200u32, 500.0);

    let lost = apply_reliability_demand(&mut s, &ridership, &revenue);
    // reliable route unchanged; unreliable route scaled down; trips shed > 0.
    assert_eq!(s.routes[0].daily_ridership, 1000.0);
    assert!(s.routes[1].daily_ridership < 1000.0);
    assert!(lost > 0.0, "chronic delay must shed transit trips");

    // approval: a fully reliable network lifts approval; a chronically late one
    // drags it. Compare the ridership-weighted approval delta both ways.
    let good = {
        let mut g = build_network(6, Difficulty::Normal);
        g.routes[0].daily_ridership = 1000.0;
        g.routes[0].on_time_pct = Some(1.0);
        g.routes[1].daily_ridership = 1000.0;
        g.routes[1].on_time_pct = Some(1.0);
        ops_approval_delta(&g)
    };
    let bad = {
        let mut b = build_network(7, Difficulty::Normal);
        b.routes[0].daily_ridership = 1000.0;
        b.routes[0].on_time_pct = Some(0.2);
        b.routes[1].daily_ridership = 1000.0;
        b.routes[1].on_time_pct = Some(0.2);
        ops_approval_delta(&b)
    };
    assert!(good > 0.0, "reliable network lifts approval, got {good}");
    assert!(bad < 0.0, "unreliable network drags approval, got {bad}");
    assert!(good <= t.approval_reliability_swing + 1e-9);
    assert!(bad >= -t.approval_reliability_swing - 1e-9);
}

#[test]
fn ops_daily_close_computes_on_time_and_dispatches_maintenance() {
    let mut s = build_network(9, Difficulty::Normal);
    // wear one metro unit below the maintenance threshold and give it a depot.
    s.depots.as_mut().unwrap().push(crate::types::Depot {
        id: 999,
        mode: TransitMode::Metro,
        pos: vec(100.0, 100.0),
        build_tick: 0,
    });
    let metro_unit = s
        .fleet
        .as_mut()
        .unwrap()
        .iter_mut()
        .find(|u| u.route_id == Some(200))
        .unwrap();
    metro_unit.condition = 0.1;
    // run a day so departures accrue, then close.
    for _ in 0..60 {
        s.tick += OPS_INTERVAL as u64;
        step(&mut s);
    }
    ops_daily_close(&mut s);
    // no incidents on the tunnel metro (weather-neutral) -> on-time ~ 1.0.
    assert!(s.routes[1].on_time_pct.unwrap() > 0.0);
    // the worn metro unit went into a maintenance window (depot exists).
    let in_maint = s
        .fleet
        .as_ref()
        .unwrap()
        .iter()
        .any(|u| u.mode == TransitMode::Metro && u.status == FleetStatus::Maintenance);
    assert!(in_maint, "worn unit with a depot should enter maintenance");
    // fleet aged one day.
    assert!(s.fleet.as_ref().unwrap().iter().all(|u| u.age_days >= 1.0));
}

#[test]
fn build_depot_validates_and_charges() {
    let mut s = build_network(3, Difficulty::Normal);
    let cash0 = s.budget.cash;
    let r = depot::build_depot(&mut s, TransitMode::Bus, vec(50.0, 50.0));
    assert!(r.ok && r.created_id.is_some());
    assert_eq!(
        s.budget.cash,
        cash0 - ops_tunables(Difficulty::Normal).depot_build_cost
    );
    // one-per-mode: a second bus depot is rejected.
    let r2 = depot::build_depot(&mut s, TransitMode::Bus, vec(60.0, 60.0));
    assert!(!r2.ok);
    // a different mode is allowed.
    let r3 = depot::build_depot(&mut s, TransitMode::Metro, vec(70.0, 70.0));
    assert!(r3.ok);
}

#[test]
fn insufficient_funds_blocks_depot() {
    let mut s = build_network(3, Difficulty::Normal);
    s.budget.cash = 100.0;
    let r = depot::build_depot(&mut s, TransitMode::Bus, vec(50.0, 50.0));
    assert!(!r.ok);
    assert_eq!(s.budget.cash, 100.0, "no charge on failure");
}

#[test]
fn peak_units_required_sizes_the_fleet() {
    let s = build_network(3, Difficulty::Normal);
    // peak (amPeak/pmPeak default 300s headway) needs the most units.
    let peak = peak_units_required(&s, &s.routes[0]);
    assert!(peak >= 1);
    // night (1200s headway) needs no more than peak.
    let night = units_for_period(&s, &s.routes[0], Period::Night);
    assert!(night <= peak);
}

#[test]
fn reliability_demand_mult_matches_ts_reference() {
    // TS reference captured via `bun` from ops/index.ts::reliabilityDemandMultFor
    // (normal difficulty -> FORGIVING: onTimeTarget 0.9, floor 0.6). Pure
    // arithmetic port, so parity is exact to f64 rounding (tol 1e-6).
    let t = ops_tunables(Difficulty::Normal);
    let cases = [
        (1.0, 1.000000),
        (0.9, 1.000000),
        (0.5, 0.822222),
        (0.0, 0.600000),
    ];
    for (on_time, ts_ref) in cases {
        let got = reliability_demand_mult_for(on_time, &t);
        assert!(
            (got - ts_ref).abs() < 1e-6,
            "on_time={on_time} got={got} ts_ref={ts_ref}"
        );
    }
}
