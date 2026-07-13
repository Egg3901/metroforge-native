//! Station / track / route build+edit+demolish command LOGIC as standalone
//! functions. Ports the real bodies from `sim/src/core/commands.ts` that P1 left
//! stubbed (`applyCommandInner` cases `buildStation`, `buildTrack`,
//! `createRoute`, `editRoute`, `demolishStation`, `demolishTrack`) plus the
//! shared helpers (`nextStationName`, `trackCost`, `stationCost`,
//! `syncVehicles`, `routePathLength`, `routeCycleSeconds`, `deriveHeadway`).
//!
//! These are NOT wired into [`crate::commands::apply_command`] (that dispatch is
//! frozen for lane isolation) — the integration owner calls them from the match.
//!
//! LANE NOTE (P3-TRANSIT): three cross-lane dependencies are not yet ported and
//! are provided here as clearly-marked neutral local stubs:
//! * `weatherEffects::weatherBuildCostMult` -> 1.0 (weather is always `None`).
//! * geology tunnel pricing (`geologyCost` strata model + station depth
//!   surcharge): tunnels are priced with the same per-metre x grade-mult x
//!   water-crossing formula as surface/elevated using the tunnel cost
//!   multiplier, WITHOUT the strata/depth surcharge. Documented simplification;
//!   the geology lane re-points `track_cost` at `trackCostDetailed`.
//! * `ops::syncFleetForRoute` -> no-op (lane B owns the fleet ledger).

use crate::commands::CommandResult;
use crate::constants::{modes, MAX_HEADWAY, REFUND_FRACTION, ROUTE_COLORS, WATER_CROSSING_MULT};
use crate::fields::is_water_at;
use crate::geometry::{dist, make_polyline, Vec2};
use crate::transit::grade_effects::{segment_day_average_speed_mps, segment_density01};
use crate::transit::road_graph::{find_road_path, nearest_road_point};
use crate::types::{
    GameState, RouteDef, Station, TrackGrade, TrackSegment, TransitMode, VehicleState,
};

/// Local `CommandResult` constructors (the ones in [`crate::commands`] are
/// private to that module). Build via the public fields.
fn r_err(msg: &str) -> CommandResult {
    CommandResult {
        ok: false,
        error: Some(msg.to_string()),
        created_id: None,
    }
}
fn r_ok() -> CommandResult {
    CommandResult {
        ok: true,
        error: None,
        created_id: None,
    }
}
fn r_created(id: u32) -> CommandResult {
    CommandResult {
        ok: true,
        error: None,
        created_id: Some(id),
    }
}

const STATION_NAMES: [&str; 42] = [
    "Central",
    "Riverside",
    "Oakwood",
    "Hillcrest",
    "Harborview",
    "Elmgate",
    "Northfield",
    "Southbank",
    "Westbrook",
    "Eastvale",
    "Maplewood",
    "Kingsway",
    "Queensport",
    "Foxhall",
    "Ironbridge",
    "Silverlake",
    "Granton",
    "Ashford",
    "Birchmount",
    "Cedarholm",
    "Drayton",
    "Everly",
    "Fairmont",
    "Glenrose",
    "Halston",
    "Inverness",
    "Juniper",
    "Kestrel",
    "Larkspur",
    "Milbourne",
    "Norcross",
    "Ottervale",
    "Pinegate",
    "Quarry",
    "Redwing",
    "Stonebridge",
    "Thornbury",
    "Uplands",
    "Vantage",
    "Wexford",
    "Yarrow",
    "Zephyr",
];

/// Next unused station name. Mirrors `nextStationName`.
fn next_station_name(state: &GameState) -> String {
    let used: std::collections::BTreeSet<&str> =
        state.stations.iter().map(|s| s.name.as_str()).collect();
    for n in STATION_NAMES {
        if !used.contains(n) {
            return n.to_string();
        }
    }
    format!("Station {}", state.stations.len() + 1)
}

/// `weatherEffects::weatherBuildCostMult` for `weather = None` -> 1.0.
fn weather_build_cost_mult(_state: &GameState) -> f64 {
    1.0
}

/// Cost of a track polyline given mode/grade. Mirrors `trackCost` for
/// surface/elevated exactly (per-metre x grade-mult x water-crossing premium x
/// weather). Tunnels use the same shape with the tunnel grade multiplier but
/// WITHOUT the geology strata / depth model (documented lane stub).
pub fn track_cost(state: &GameState, mode: TransitMode, grade: TrackGrade, points: &[Vec2]) -> f64 {
    let cfg = modes(mode);
    let per_meter = cfg.track_cost_per_meter * cfg.grade_cost_mult.get(grade);
    let mut cost = 0.0;
    for i in 1..points.len() {
        let a = points[i - 1];
        let b = points[i];
        let len = dist(a, b);
        let samples = ((len / 120.0).ceil() as i64).max(2);
        let mut water_frac = 0.0;
        for s in 0..=samples {
            let t = s as f64 / samples as f64;
            let p = Vec2 {
                x: a.x + (b.x - a.x) * t,
                y: a.y + (b.y - a.y) * t,
            };
            if is_water_at(&state.fields, p) {
                water_frac += 1.0 / (samples as f64 + 1.0);
            }
        }
        let water_mult = 1.0 + water_frac * (WATER_CROSSING_MULT - 1.0);
        cost += len * per_meter * water_mult;
    }
    let weather_mult = if grade == TrackGrade::Tunnel {
        1.0
    } else {
        weather_build_cost_mult(state)
    };
    (cost * weather_mult).round()
}

/// Cost to place one station. Mirrors `stationCost`.
pub fn station_cost(mode: TransitMode) -> f64 {
    modes(mode).station_cost
}

/// Place a new station. Mirrors `applyCommandInner` case `buildStation`.
pub fn build_station(state: &mut GameState, mode: TransitMode, pos: Vec2) -> CommandResult {
    if !state.unlocked_modes.contains(&mode) {
        return r_err(&format!("{} not yet unlocked", modes(mode).label));
    }
    let mut pos = pos;
    if mode == TransitMode::Bus || mode == TransitMode::Tram {
        if let Some(snapped) = nearest_road_point(&state.roads, pos, 260.0) {
            pos = snapped;
        }
    }
    if is_water_at(&state.fields, pos) {
        return r_err("Cannot build a station on water");
    }
    let cost = station_cost(mode);
    if state.budget.cash < cost {
        return r_err("Insufficient funds");
    }
    for s in &state.stations {
        if s.mode == mode && dist(s.pos, pos) < 200.0 {
            return r_err("Too close to an existing station of this mode");
        }
    }
    let id = state.next_id;
    state.next_id += 1;
    let name = next_station_name(state);
    state.stations.push(Station {
        id,
        name,
        pos,
        mode,
        level: 1,
        ridership: 0.0,
        alightings: 0.0,
        build_tick: state.tick,
        depth: None,
    });
    state.budget.cash -= cost;
    state.demand_dirty = true;
    state.stats.approval = (state.stats.approval + 2.0).min(100.0);
    r_created(id)
}

/// Build a track between two stations. Mirrors case `buildTrack`. The tunnel
/// station-deepening surcharge is skipped (geology lane stub); depth stays
/// `None`.
pub fn build_track(
    state: &mut GameState,
    mode: TransitMode,
    grade: TrackGrade,
    from_station_id: u32,
    to_station_id: u32,
    waypoints: &[Vec2],
) -> CommandResult {
    let Some(from) = state
        .stations
        .iter()
        .find(|s| s.id == from_station_id)
        .cloned()
    else {
        return r_err("Station not found");
    };
    let Some(to) = state
        .stations
        .iter()
        .find(|s| s.id == to_station_id)
        .cloned()
    else {
        return r_err("Station not found");
    };
    if from.id == to.id {
        return r_err("Track must connect two different stations");
    }
    if from.mode != mode || to.mode != mode {
        return r_err("Both stations must match the track mode");
    }
    if !modes(mode).grade_options.contains(&grade) {
        return r_err(&format!("{} cannot be built here", modes(mode).label));
    }
    let exists = state.tracks.iter().any(|t| {
        t.mode == mode
            && ((t.from_station_id == from.id && t.to_station_id == to.id)
                || (t.from_station_id == to.id && t.to_station_id == from.id))
    });
    if exists {
        return r_err("Track already exists between these stations");
    }
    let mut points: Vec<Vec2> = {
        let mut v = vec![from.pos];
        v.extend_from_slice(waypoints);
        v.push(to.pos);
        v
    };
    if mode == TransitMode::Bus || mode == TransitMode::Tram {
        let mut stops = vec![from.pos];
        stops.extend_from_slice(waypoints);
        stops.push(to.pos);
        let mut routed: Vec<Vec2> = Vec::new();
        let mut all_found = true;
        for i in 0..stops.len().saturating_sub(1) {
            match find_road_path(&state.roads, stops[i], stops[i + 1]) {
                None => {
                    all_found = false;
                    break;
                }
                Some(leg) => {
                    let skip = if routed.is_empty() { 0 } else { 1 };
                    for p in leg.into_iter().skip(skip) {
                        routed.push(p);
                    }
                }
            }
        }
        if all_found && routed.len() >= 2 {
            points = routed;
        }
    }
    let cost = track_cost(state, mode, grade, &points);
    if state.budget.cash < cost {
        return r_err("Insufficient funds");
    }
    let id = state.next_id;
    state.next_id += 1;
    let mut seg = TrackSegment {
        id,
        mode,
        grade,
        from_station_id: from.id,
        to_station_id: to.id,
        polyline: make_polyline(points),
        build_cost: cost,
        congestion_density: None,
    };
    seg.congestion_density = Some(segment_density01(Some(&state.fields), &seg));
    state.tracks.push(seg);
    state.budget.cash -= cost;
    state.demand_dirty = true;
    r_created(id)
}

/// Create a route through the given stations. Mirrors case `createRoute`.
/// `syncFleetForRoute` is skipped (lane B owns the fleet ledger).
pub fn create_route(
    state: &mut GameState,
    mode: TransitMode,
    station_ids: &[u32],
) -> CommandResult {
    if station_ids.len() < 2 {
        return r_err("A route needs at least 2 stops");
    }
    let mut segment_ids: Vec<u32> = Vec::new();
    for i in 0..station_ids.len() - 1 {
        let a = station_ids[i];
        let b = station_ids[i + 1];
        let seg = state.tracks.iter().find(|t| {
            t.mode == mode
                && ((t.from_station_id == a && t.to_station_id == b)
                    || (t.from_station_id == b && t.to_station_id == a))
        });
        match seg {
            None => {
                return r_err(&format!(
                    "No {} track between stops {} and {}",
                    modes(mode).label,
                    i + 1,
                    i + 2
                ))
            }
            Some(s) => segment_ids.push(s.id),
        }
    }
    let cfg = modes(mode);
    let id = state.next_id;
    state.next_id += 1;
    let color = ROUTE_COLORS[state.routes.len() % ROUTE_COLORS.len()].to_string();
    let same_mode_count = state.routes.iter().filter(|r| r.mode == mode).count();
    state.routes.push(RouteDef {
        id,
        name: format!("{} {}", cfg.label, same_mode_count + 1),
        color,
        mode,
        station_ids: station_ids.to_vec(),
        segment_ids,
        headway_seconds: cfg.default_headway,
        fare: 2.5,
        vehicle_count: 0,
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
    });
    // starter fleet: 2 vehicles if affordable.
    let starter_cost = 2.0 * cfg.vehicle_cost;
    if state.budget.cash >= starter_cost {
        state.budget.cash -= starter_cost;
        if let Some(route) = state.routes.last_mut() {
            route.vehicle_count = 2;
        }
        sync_vehicles(state, id);
        // TODO(lane-B ops): syncFleetForRoute(state, id).
    }
    let hw = derive_headway(state, id);
    if let Some(created) = state.routes.iter_mut().find(|r| r.id == id) {
        created.headway_seconds = hw;
        created.scheduled_headway = Some(hw);
    }
    state.demand_dirty = true;
    r_created(id)
}

/// Edit mutable route properties. Mirrors case `editRoute`. `syncFleetForRoute`
/// is skipped (lane B).
#[allow(clippy::too_many_arguments)]
pub fn edit_route(
    state: &mut GameState,
    route_id: u32,
    fare: Option<f64>,
    name: Option<&str>,
    color: Option<&str>,
    vehicle_count: Option<u32>,
) -> CommandResult {
    let Some(idx) = state.routes.iter().position(|r| r.id == route_id) else {
        return r_err("Route not found");
    };
    let mode = state.routes[idx].mode;
    let cfg = modes(mode);
    if let Some(f) = fare {
        state.routes[idx].fare = f.clamp(0.0, 10.0);
    }
    if let Some(n) = name {
        state.routes[idx].name = n.chars().take(40).collect();
    }
    if let Some(c) = color {
        state.routes[idx].color = c.to_string();
    }
    if let Some(vc) = vehicle_count {
        let target = vc.min(40);
        let current = state.routes[idx].vehicle_count;
        if target > current {
            let cost = (target - current) as f64 * cfg.vehicle_cost;
            if state.budget.cash < cost {
                return r_err("Insufficient funds for vehicles");
            }
            state.budget.cash -= cost;
        } else if target < current {
            state.budget.cash += (current - target) as f64 * cfg.vehicle_cost * 0.4;
        }
        state.routes[idx].vehicle_count = target;
        sync_vehicles(state, route_id);
        // TODO(lane-B ops): syncFleetForRoute(state, route_id).
    }
    let hw = derive_headway(state, route_id);
    if let Some(route) = state.routes.iter_mut().find(|r| r.id == route_id) {
        route.headway_seconds = hw;
        route.scheduled_headway = Some(hw);
    }
    state.demand_dirty = true;
    r_ok()
}

/// Demolish a station by id. Mirrors case `demolishStation`.
pub fn demolish_station(state: &mut GameState, station_id: u32) -> CommandResult {
    let Some(idx) = state.stations.iter().position(|s| s.id == station_id) else {
        return r_err("Station not found");
    };
    if state
        .routes
        .iter()
        .any(|r| r.station_ids.contains(&station_id))
    {
        return r_err("Remove routes serving this station first");
    }
    if state
        .tracks
        .iter()
        .any(|t| t.from_station_id == station_id || t.to_station_id == station_id)
    {
        return r_err("Demolish connected tracks first");
    }
    let mode = state.stations[idx].mode;
    state.budget.cash += modes(mode).station_cost * REFUND_FRACTION;
    state.stations.remove(idx);
    state.demand_dirty = true;
    r_ok()
}

/// Demolish a track by id. Mirrors case `demolishTrack`.
pub fn demolish_track(state: &mut GameState, track_id: u32) -> CommandResult {
    let Some(idx) = state.tracks.iter().position(|t| t.id == track_id) else {
        return r_err("Track not found");
    };
    if state
        .routes
        .iter()
        .any(|r| r.segment_ids.contains(&track_id))
    {
        return r_err("Remove routes using this track first");
    }
    state.budget.cash += state.tracks[idx].build_cost * REFUND_FRACTION;
    state.tracks.remove(idx);
    state.demand_dirty = true;
    r_ok()
}

// ── Route geometry helpers (ported from commands.ts) ──────────────────────────

/// Rebuild the vehicle pool for a route, spacing vehicles evenly. Mirrors
/// `syncVehicles`.
pub fn sync_vehicles(state: &mut GameState, route_id: u32) {
    let Some(route) = state.routes.iter().find(|r| r.id == route_id) else {
        return;
    };
    let vehicle_count = route.vehicle_count;
    state.vehicles.retain(|v| v.route_id != route_id);
    let path_length = route_path_length(state, route_id);
    if path_length <= 0.0 {
        return;
    }
    for i in 0..vehicle_count {
        let id = state.next_id;
        state.next_id += 1;
        state.vehicles.push(VehicleState {
            id,
            route_id,
            along: (i as f64 / vehicle_count as f64) * path_length,
            path_length,
            dwell_remaining: 0.0,
            occupancy: 0.0,
        });
    }
}

/// Out-and-back path length for a route. Mirrors `routePathLength`.
pub fn route_path_length(state: &GameState, route_id: u32) -> f64 {
    let Some(route) = state.routes.iter().find(|r| r.id == route_id) else {
        return 0.0;
    };
    let mut one_way = 0.0;
    for &seg_id in &route.segment_ids {
        if let Some(seg) = state.tracks.iter().find(|t| t.id == seg_id) {
            one_way += seg.polyline.length;
        }
    }
    one_way * 2.0
}

/// Seconds for one vehicle to complete a full out-and-back cycle. Mirrors
/// `routeCycleSeconds`.
pub fn route_cycle_seconds(state: &GameState, route_id: u32) -> f64 {
    let Some(route) = state.routes.iter().find(|r| r.id == route_id) else {
        return 0.0;
    };
    let cfg = modes(route.mode);
    let mut one_way = 0.0;
    for &seg_id in &route.segment_ids {
        if let Some(seg) = state.tracks.iter().find(|t| t.id == seg_id) {
            let dens = segment_density01(Some(&state.fields), seg);
            let spd = segment_day_average_speed_mps(route.mode, seg.grade, dens);
            if spd > 0.0 {
                one_way += seg.polyline.length / spd;
            }
        }
    }
    if one_way <= 0.0 {
        return 0.0;
    }
    let travel = one_way * 2.0;
    let dwell_stops = 2.0 * (route.station_ids.len().saturating_sub(1)).max(1) as f64;
    travel + dwell_stops * cfg.dwell_seconds
}

/// Headway as a consequence of fleet size. Mirrors `deriveHeadway`.
pub fn derive_headway(state: &GameState, route_id: u32) -> f64 {
    let Some(route) = state.routes.iter().find(|r| r.id == route_id) else {
        return MAX_HEADWAY;
    };
    let cfg = modes(route.mode);
    if route.vehicle_count == 0 {
        return MAX_HEADWAY;
    }
    let cycle = route_cycle_seconds(state, route_id);
    if cycle <= 0.0 {
        return cfg.default_headway;
    }
    (cycle / route.vehicle_count as f64).clamp(cfg.min_headway, MAX_HEADWAY)
}
