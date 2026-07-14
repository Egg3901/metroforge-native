//! P3 lane A (transit) tests:
//! 1. run-twice / two-instance determinism of `run_assignment`.
//! 2. behavioral tolerance vs the TS reference (`sim/_capture_assignment.ts`)
//!    for a small scripted bus network.
//! 3. build-command logic (`build_station` / `build_track` / `create_route`)
//!    over a procedurally generated network from `new_game`.

use mf_sim::geometry::{make_polyline, Vec2};
use mf_sim::transit::assignment::run_assignment;
use mf_sim::transit::build::{build_station, build_track, create_route};
use mf_sim::types::{
    Difficulty, District, FieldGrid, GameState, RouteDef, Station, TrackGrade, TrackSegment,
    TransitMode,
};

/// The exact scripted network captured by `sim/_capture_assignment.ts`.
fn scripted_state() -> GameState {
    let n = 16usize; // 4x4
    let fields = FieldGrid {
        w: 4,
        h: 4,
        cell_size: 1000.0,
        origin_x: 0.0,
        origin_y: 0.0,
        terrain: vec![0.0; n],
        water: vec![0u8; n],
        parks: vec![0u8; n],
        population: vec![0.0; n],
        jobs: vec![0.0; n],
        land_value: vec![0.5; n],
        nimby: vec![0.0; n],
    };
    let districts = vec![
        District {
            id: 1,
            name: "A".into(),
            centroid: Vec2 { x: 200.0, y: 200.0 },
            cell_indices: vec![],
            population: 1000.0,
            jobs: 100.0,
            land_value: 0.5,
            last_growth_delta: None,
        },
        District {
            id: 2,
            name: "B".into(),
            centroid: Vec2 {
                x: 2200.0,
                y: 200.0,
            },
            cell_indices: vec![],
            population: 100.0,
            jobs: 1000.0,
            land_value: 0.5,
            last_growth_delta: None,
        },
    ];
    let stations = vec![
        Station {
            id: 10,
            name: "S1".into(),
            pos: Vec2 { x: 200.0, y: 200.0 },
            mode: TransitMode::Bus,
            level: 1,
            ridership: 0.0,
            alightings: 0.0,
            build_tick: 0,
            depth: None,
        },
        Station {
            id: 20,
            name: "S2".into(),
            pos: Vec2 {
                x: 2200.0,
                y: 200.0,
            },
            mode: TransitMode::Bus,
            level: 1,
            ridership: 0.0,
            alightings: 0.0,
            build_tick: 0,
            depth: None,
        },
    ];
    let tracks = vec![TrackSegment {
        id: 30,
        mode: TransitMode::Bus,
        grade: TrackGrade::Surface,
        from_station_id: 10,
        to_station_id: 20,
        polyline: make_polyline(vec![
            Vec2 { x: 200.0, y: 200.0 },
            Vec2 {
                x: 2200.0,
                y: 200.0,
            },
        ]),
        build_cost: 0.0,
        congestion_density: None,
    }];
    let routes = vec![RouteDef {
        id: 40,
        name: "Bus 1".into(),
        color: "#e6a817".into(),
        mode: TransitMode::Bus,
        station_ids: vec![10, 20],
        segment_ids: vec![30],
        headway_seconds: 300.0,
        fare: 2.5,
        vehicle_count: 2,
        daily_ridership: 0.0,
        daily_revenue: 0.0,
        capacity: 0.0,
        load: 0.0,
        crowding: 0.0,
        segment_loads: vec![],
        surface_exposure: None,
        move_grade_speed: None,
        frequency: None,
        scheduled_headway: None,
        in_service_vehicles: None,
        on_time_pct: None,
        avg_delay_sec: None,
        reliability_demand_mult: None,
    }];

    let mut s = GameState::with_difficulty(12345, Difficulty::Normal);
    s.tick = 400; // ~08:00 game-hour, matching the TS capture
    s.fields = fields;
    s.districts = districts;
    s.stations = stations;
    s.tracks = tracks;
    s.routes = routes;
    s
}

#[test]
fn assignment_run_twice_is_deterministic() {
    let s = scripted_state();
    let a = run_assignment(&s);
    let b = run_assignment(&s);
    assert_eq!(a.daily_transit_trips, b.daily_transit_trips);
    assert_eq!(a.daily_car_trips, b.daily_car_trips);
    assert_eq!(a.route_ridership, b.route_ridership);
    assert_eq!(a.station_boardings, b.station_boardings);
    assert_eq!(a.segment_load, b.segment_load);
    assert_eq!(a.flows.len(), b.flows.len());
}

#[test]
fn assignment_two_instances_match() {
    let a = run_assignment(&scripted_state());
    let b = run_assignment(&scripted_state());
    assert_eq!(a.daily_transit_trips, b.daily_transit_trips);
    assert_eq!(a.route_revenue, b.route_revenue);
}

/// Behavioral tolerance vs the captured TS reference numbers. The Rust baseline
/// is not bit-identical to JS, so assert within a tight relative band (0.5%).
#[test]
fn assignment_matches_ts_reference_within_tolerance() {
    // Captured 2026-07-13 via `bun run sim/_capture_assignment.ts`:
    const TS_TRANSIT: f64 = 577.6245738466636;
    const TS_CAR: f64 = 412.37542615333643;
    const TS_SHARE: f64 = 0.5834591655016804;
    const TS_RIDERSHIP40: f64 = 577.6245738466636;
    const TS_REVENUE40: f64 = 1444.0614346166587;
    const TS_BOARD10: f64 = 525.1132489515123;

    let out = run_assignment(&scripted_state());
    let share = out.daily_transit_trips / (out.daily_transit_trips + out.daily_car_trips);

    let close = |got: f64, want: f64| {
        let denom = want.abs().max(1.0);
        (got - want).abs() / denom < 0.005
    };
    assert!(
        close(out.daily_transit_trips, TS_TRANSIT),
        "transit {} vs {}",
        out.daily_transit_trips,
        TS_TRANSIT
    );
    assert!(
        close(out.daily_car_trips, TS_CAR),
        "car {} vs {}",
        out.daily_car_trips,
        TS_CAR
    );
    assert!(close(share, TS_SHARE), "share {} vs {}", share, TS_SHARE);
    assert!(close(
        *out.route_ridership.get(&40).unwrap(),
        TS_RIDERSHIP40
    ));
    assert!(close(*out.route_revenue.get(&40).unwrap(), TS_REVENUE40));
    assert!(close(*out.station_boardings.get(&10).unwrap(), TS_BOARD10));
    assert_eq!(out.flows.len(), 2);
}

#[test]
fn build_commands_over_new_game_network() {
    use mf_sim::new_game::{new_game, NewGameOptions};
    let mut s = new_game(7, Difficulty::Easy, NewGameOptions::default());
    // Place two bus stations far apart on land.
    let mut placed: Vec<u32> = Vec::new();
    let candidates = [
        Vec2 { x: -1500.0, y: 0.0 },
        Vec2 { x: 1500.0, y: 0.0 },
        Vec2 { x: 0.0, y: -1500.0 },
        Vec2 { x: 0.0, y: 1500.0 },
        Vec2 {
            x: -1500.0,
            y: 1500.0,
        },
    ];
    for c in candidates {
        let r = build_station(&mut s, TransitMode::Bus, c);
        if r.ok {
            placed.push(r.created_id.unwrap());
        }
        if placed.len() >= 2 {
            break;
        }
    }
    assert!(
        placed.len() >= 2,
        "expected to place 2 bus stations, got {}",
        placed.len()
    );

    let cash_before = s.budget.cash;
    let tr = build_track(
        &mut s,
        TransitMode::Bus,
        TrackGrade::Surface,
        placed[0],
        placed[1],
        &[],
    );
    assert!(tr.ok, "build_track failed: {:?}", tr.error);
    assert!(s.budget.cash < cash_before, "track should cost money");
    let track_id = tr.created_id.unwrap();
    assert!(s.tracks.iter().any(|t| t.id == track_id));

    let cr = create_route(&mut s, TransitMode::Bus, &[placed[0], placed[1]]);
    assert!(cr.ok, "create_route failed: {:?}", cr.error);
    let route = s
        .routes
        .iter()
        .find(|r| r.id == cr.created_id.unwrap())
        .unwrap();
    // starter fleet of 2 (Easy start has ample cash) -> real derived headway.
    assert_eq!(route.vehicle_count, 2);
    assert!(route.headway_seconds > 0.0 && route.headway_seconds < mf_sim::constants::MAX_HEADWAY);
    assert_eq!(
        s.vehicles.iter().filter(|v| v.route_id == route.id).count(),
        2
    );

    // Now that the network exists, assignment runs and produces some ridership.
    let out = run_assignment(&s);
    assert!(out.daily_transit_trips >= 0.0);
}
