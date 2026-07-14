//! The fixed-timestep tick orchestrator. Port of `sim/src/core/sim.ts`
//! (`simTick`, ~687 lines). 1 tick = 1 game-second. Everything here is
//! deterministic given `(seed, command stream)`.
//!
//! This wires the P3 lane systems (transit assignment + vehicle movement,
//! v0.9 operations, weather + weather effects, events, derived analytics,
//! scenario) into one faithful `sim_tick` that matches the ORDER systems run in
//! `sim.ts`. The determinism contract is the NEW RUST BASELINE: run-twice
//! identical `state_hash`, plus behavioral tolerance vs the TS reference.

use crate::analytics;
use crate::constants::{
    base_daily_subsidy, grade_maint_mult, modes, ASSIGNMENT_INTERVAL_TICKS, BANKRUPTCY_FLOOR,
    BANKRUPTCY_GRACE_DAYS, CROWD_APPROVAL_THRESHOLD, GROWTH_INTERVAL_DAYS, PEAK_HOUR_FRACTION,
    TICKS_PER_DAY,
};
use crate::events::{
    event_approval_delta, event_by_id, event_fare_mult, roll_event, ActiveEvent, EventTone,
};
use crate::fields::cell_center;
use crate::geometry::{dist, Polyline, Vec2};
use crate::ops;
use crate::rng::Rng;
use crate::scenario;
use crate::transit::assignment::{run_assignment, CarFlow};
use crate::transit::grade_effects::{segment_day_average_speed_mps, segment_density01};
use crate::transit::route_path::get_route_path;
use crate::transit::time_of_day::{diurnal_demand, diurnal_factor};
use crate::transit::traffic::compute_traffic;
use crate::types::{GameState, TransitMode};
use crate::weather::{climate_table, weather_at, WeatherEvent};
use crate::weather_effects::weather_speed_mult;

/// Ticks per game-hour: weather is refreshed at most this often (a cheap pure
/// function of the tick). `TICKS_PER_DAY / 24`.
const TICKS_PER_HOUR: u64 = TICKS_PER_DAY as u64 / 24;

/// Lose if approval stays at/below the floor for this many days. Mirrors
/// `APPROVAL_GRACE_DAYS` (scenarioRules.ts).
const APPROVAL_GRACE_DAYS: u32 = 5;

/// UI toast tone. Mirrors the `tone` union on `TickEvents.toasts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastTone {
    /// Positive.
    Good,
    /// Warning / negative.
    Warn,
    /// Neutral / informational.
    Info,
}

/// A themed toast (no em/en dashes in copy).
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    /// Player-facing message.
    pub message: String,
    /// Tone.
    pub tone: ToastTone,
}

/// Why the run ended this tick (subset of [`crate::types::FailReason`] the tick
/// surfaces to the host).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickFail {
    /// Approval floor breach.
    Approval,
    /// Scenario day limit passed.
    Time,
    /// Scenario lose condition.
    Condition,
}

/// Outputs of one tick for the host. Mirrors `TickEvents`.
#[derive(Clone, Debug, Default)]
pub struct TickEvents {
    /// Sim-day just completed (on a day boundary).
    pub day_completed: Option<i64>,
    /// Went bankrupt this tick.
    pub bankrupt: bool,
    /// Failure surfaced this tick.
    pub failed: Option<TickFail>,
    /// Scenario win tree satisfied.
    pub won: bool,
    /// A mode unlocked (its label).
    pub mode_unlocked: Option<String>,
    /// Plain-language messages.
    pub messages: Vec<String>,
    /// Themed toasts.
    pub toasts: Vec<Toast>,
    /// Optional ridership heatmap (emitted on the analytics cadence).
    pub heatmap: Option<analytics::HeatmapPayload>,
    /// True when assignment/overlay state refreshed this tick.
    pub assignment_refreshed: bool,
}

/// Player copy for weather-event toasts. No em/en dashes, no filler. Mirrors
/// `WEATHER_EVENT_COPY`.
fn weather_event_copy(ev: WeatherEvent) -> (&'static str, &'static str, ToastTone) {
    match ev {
        WeatherEvent::Blizzard => (
            "Blizzard warning. Surface lines are crawling, but the underground keeps moving.",
            "The blizzard has passed. Surface service is recovering.",
            ToastTone::Warn,
        ),
        WeatherEvent::Heatwave => (
            "Heat wave. Riders are staying home and rail speeds are restricted.",
            "The heat wave has broken. Rail speed limits are lifted.",
            ToastTone::Warn,
        ),
    }
}

/// Refresh the cached sky and emit begin/end toasts when a headline weather
/// event starts or clears. Mirrors `updateWeather`.
fn update_weather(state: &mut GameState, events: &mut TickEvents) {
    let table = climate_table(state.city_key.as_deref());
    let next = weather_at(state.seed, state.tick, &table);
    let prev_event = state.last_weather_event;
    let next_event = next.event;
    state.weather = Some(next);
    if next_event != prev_event {
        if let Some(pe) = prev_event {
            let (_s, end, _t) = weather_event_copy(pe);
            events.toasts.push(Toast {
                message: end.to_string(),
                tone: ToastTone::Info,
            });
        }
        if let Some(ne) = next_event {
            let (start, _e, tone) = weather_event_copy(ne);
            events.toasts.push(Toast {
                message: start.to_string(),
                tone,
            });
        }
        state.last_weather_event = next_event;
    }
}

/// `routeOperatingCost` (economy.ts): ops + maintenance per vehicle per day.
fn route_operating_cost(mode: TransitMode, vehicle_count: u32) -> f64 {
    let cfg = modes(mode);
    vehicle_count as f64 * (cfg.ops_per_vehicle_per_day + cfg.maint_per_vehicle_per_day)
}

/// `modeUnlockReady` (scenarioRules.ts).
fn mode_unlock_ready(mode: TransitMode, stats: &crate::types::CityStats) -> bool {
    match mode {
        TransitMode::Bus => true,
        TransitMode::Tram => stats.daily_transit_trips >= 1_000.0 || stats.population >= 50_000.0,
        TransitMode::Metro => {
            stats.transit_share >= 0.1 || stats.coverage >= 0.5 || stats.population >= 150_000.0
        }
        TransitMode::Rail => {
            stats.transit_share >= 0.25
                || stats.daily_transit_trips >= 50_000.0
                || stats.population >= 300_000.0
        }
    }
}

/// Advance the simulation by one tick. Mirrors `simTick` (sim.ts:164). The tick
/// ORDER matches the TS source exactly (see the numbered comments).
pub fn sim_tick(state: &mut GameState) -> TickEvents {
    let mut events = TickEvents::default();
    if state.failed.is_some() || state.scenario_won == Some(true) {
        return events;
    }
    state.tick += 1;

    // 1. refresh the sky once per game-hour (and on the very first tick)
    if state.tick.is_multiple_of(TICKS_PER_HOUR) || state.weather.is_none() {
        update_weather(state, &mut events);
    }

    // 2. per-tick vehicle movement / positions
    move_vehicles(state);

    // 3. v0.9 operations: per-period frequency, condition decay, breakdowns,
    //    reliability accrual. Runs on the fixed OPS_INTERVAL grid.
    if ops::ops_due(state.tick) {
        let r = ops::step(state);
        for t in r.toasts {
            events.toasts.push(Toast {
                message: t.message,
                tone: match t.tone {
                    ops::OpsTone::Good => ToastTone::Good,
                    ops::OpsTone::Warn => ToastTone::Warn,
                    ops::OpsTone::Info => ToastTone::Info,
                },
            });
        }
    }

    // 4. demand assignment: on dirty flag or the periodic refresh
    if state.demand_dirty || state.tick.is_multiple_of(ASSIGNMENT_INTERVAL_TICKS as u64) {
        refresh_assignment(state);
        state.demand_dirty = false;
        events.assignment_refreshed = true;
    }

    // 5. daily boundary
    if state.tick.is_multiple_of(TICKS_PER_DAY as u64) {
        let day = (state.tick / TICKS_PER_DAY as u64) as i64;
        events.day_completed = Some(day);
        update_events(state, day, &mut events);
        run_daily_economy(state, &mut events);
        // ops day-close BEFORE approval so the reliability term is fresh.
        ops::ops_daily_close(state);
        update_approval(state);
        check_unlocks(state, &mut events);
        if day % GROWTH_INTERVAL_DAYS as i64 == 0 {
            run_growth(state);
        }
        // analytics day-close: rolling heatmap/OD + optional payload
        if let Some(payload) = analytics::commit_analytics_day(state, day) {
            events.heatmap = Some(payload);
        }
        if let Some(def) = state
            .scenario
            .as_ref()
            .and_then(|s| scenario::catalog::playable_scenario(&s.id))
        {
            let sr = scenario::evaluate_scenario_day(state, def, day);
            events.messages.extend(sr.messages);
            for t in sr.toasts {
                events.toasts.push(Toast {
                    message: t.message,
                    tone: match t.tone {
                        scenario::events::ScenarioTone::Info => ToastTone::Info,
                        scenario::events::ScenarioTone::Warn => ToastTone::Warn,
                        scenario::events::ScenarioTone::Good => ToastTone::Good,
                    },
                });
            }
            if sr.won {
                events.won = true;
            }
            if sr.lost_condition {
                events.failed = Some(TickFail::Condition);
            }
        }
        check_failure(state, day, &mut events);
    }

    events
}

// ── failure / bankruptcy ──────────────────────────────────────────────────────

fn check_failure(state: &mut GameState, day: i64, events: &mut TickEvents) {
    if state.budget.cash < BANKRUPTCY_FLOOR {
        state.bankrupt_days += 1;
        if state.bankrupt_days >= BANKRUPTCY_GRACE_DAYS {
            state.failed = Some(crate::types::FailReason::Bankrupt);
            events.bankrupt = true;
        } else {
            events.messages.push(format!(
                "Deep in the red: {} days until the city takes over",
                BANKRUPTCY_GRACE_DAYS - state.bankrupt_days
            ));
        }
    } else {
        state.bankrupt_days = 0;
    }
    if state.failed.is_some() {
        return;
    }

    if let Some(floor) = state.scenario_rules.as_ref().and_then(|r| r.approval_floor) {
        if state.stats.approval <= floor {
            state.low_approval_days += 1;
            if state.low_approval_days >= APPROVAL_GRACE_DAYS {
                state.failed = Some(crate::types::FailReason::Approval);
                events.failed = Some(TickFail::Approval);
                events
                    .messages
                    .push("Approval collapsed - the board has fired you".to_string());
            } else {
                events.messages.push(format!(
                    "Approval critical ({}%): {} days to turn it around",
                    state.stats.approval.round(),
                    APPROVAL_GRACE_DAYS - state.low_approval_days
                ));
            }
        } else {
            state.low_approval_days = 0;
        }
    }
    if state.failed.is_some() {
        return;
    }
    if state.scenario_won == Some(true) {
        return;
    }
    if let Some(max_day) = state.scenario_rules.as_ref().and_then(|r| r.max_day) {
        if day > max_day as i64 {
            state.failed = Some(crate::types::FailReason::Time);
            events.failed = Some(TickFail::Time);
            events.messages.push(format!(
                "Time is up - day {max_day} has passed without meeting the objective"
            ));
        }
    }
}

// ── vehicle movement ──────────────────────────────────────────────────────────

/// Precomputed per-route movement data shared by all its vehicles this tick.
struct RouteMove {
    mode: TransitMode,
    path: Polyline,
    stops: Vec<f64>,
    move_grade_speed: f64,
    surface_exposure: f64,
    crowding: f64,
    segment_loads: Vec<f64>,
    capacity: f64,
    station_count: usize,
    vehicle_count: u32,
}

fn move_vehicles(state: &mut GameState) {
    let tod = diurnal_factor(state.tick);
    let weather = state.weather;

    // Precompute route path + stop distances (immutable borrow) BEFORE mutating
    // the vehicles. Mirrors the TS per-tick memoization.
    let station_pos: std::collections::BTreeMap<u32, Vec2> =
        state.stations.iter().map(|s| (s.id, s.pos)).collect();
    let mut route_moves: std::collections::BTreeMap<u32, RouteMove> =
        std::collections::BTreeMap::new();
    for r in &state.routes {
        let Some(path) = get_route_path(state, r) else {
            continue;
        };
        let stops = all_stop_distances(&path, &r.station_ids, &station_pos);
        route_moves.insert(
            r.id,
            RouteMove {
                mode: r.mode,
                move_grade_speed: r.move_grade_speed.unwrap_or(modes(r.mode).speed),
                surface_exposure: r.surface_exposure.unwrap_or(1.0),
                crowding: r.crowding,
                segment_loads: r.segment_loads.clone(),
                capacity: r.capacity,
                station_count: r.station_ids.len(),
                vehicle_count: r.vehicle_count,
                path,
                stops,
            },
        );
    }

    for v in state.vehicles.iter_mut() {
        let Some(rm) = route_moves.get(&v.route_id) else {
            continue;
        };
        let path_len = rm.path.length;
        v.path_length = path_len;
        if v.dwell_remaining > 0.0 {
            v.dwell_remaining -= 1.0;
            v.occupancy = occupancy_at(rm, v.along, path_len, tod);
            continue;
        }
        let cfg = modes(rm.mode);
        let weather_mult = weather_speed_mult(weather.as_ref(), rm.mode, rm.surface_exposure);
        let mut remaining = rm.move_grade_speed * weather_mult;
        let mut guard = 0;
        while remaining > 1e-6 && guard < 8 {
            guard += 1;
            let next_stop = next_stop_ahead(&rm.stops, v.along, path_len);
            let gap = match next_stop {
                None => remaining,
                Some(ns) => {
                    if ns >= v.along {
                        ns - v.along
                    } else {
                        path_len - v.along + ns
                    }
                }
            };
            if let Some(ns) = next_stop {
                if gap <= remaining + 1e-6 {
                    v.along = ns % path_len;
                    v.dwell_remaining = cfg.dwell_seconds;
                    break;
                }
            }
            let step = remaining.min(gap);
            v.along = (v.along + step) % path_len;
            remaining -= step;
        }
        v.occupancy = occupancy_at(rm, v.along, path_len, tod);
    }
}

/// Per-vehicle load from the segment the vehicle is on. Mirrors `occupancyAt`.
fn occupancy_at(rm: &RouteMove, along: f64, path_len: f64, tod_factor: f64) -> f64 {
    if rm.vehicle_count == 0 {
        return 0.0;
    }
    let n = (rm.station_count.saturating_sub(1)).max(1);
    if rm.segment_loads.is_empty() || path_len <= 0.0 || rm.capacity <= 0.0 {
        return (rm.crowding.max(0.0) * tod_factor).min(1.5);
    }
    let half = path_len / 2.0;
    let seg_idx = if along <= half {
        ((along / half) * n as f64).floor() as usize
    } else {
        let t = (along - half) / half;
        (n as f64 - 1.0 - (t * n as f64).floor()).max(0.0) as usize
    }
    .min(n - 1);
    let load = rm.segment_loads.get(seg_idx).copied().unwrap_or(0.0);
    let peak = load * 0.14;
    ((peak / rm.capacity) * tod_factor).min(1.5)
}

/// Distances along an out-and-back path where it passes near each station.
/// Mirrors `allStopDistances`.
fn all_stop_distances(
    path: &Polyline,
    station_ids: &[u32],
    station_pos: &std::collections::BTreeMap<u32, Vec2>,
) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    for sid in station_ids {
        let Some(pos) = station_pos.get(sid) else {
            continue;
        };
        for i in 0..path.points.len() {
            if dist(path.points[i], *pos) < 30.0 {
                out.push(path.cumulative[i]);
            }
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut uniq: Vec<f64> = Vec::new();
    for d in out {
        if uniq.is_empty() || (d - *uniq.last().unwrap()).abs() > 5.0 {
            uniq.push(d);
        }
    }
    uniq
}

/// Next stop strictly ahead (wraps to first). Mirrors `nextStopAhead`.
fn next_stop_ahead(stops: &[f64], along: f64, path_len: f64) -> Option<f64> {
    if stops.is_empty() || path_len <= 0.0 {
        return None;
    }
    for &d in stops {
        if d > along + 0.5 {
            return Some(d);
        }
    }
    stops.first().copied()
}

// ── assignment refresh (post-processing) ──────────────────────────────────────

/// Coarse spatial bucket over stations for coverage/growth. Mirrors
/// `StationGrid`. `candidates` returns ASCENDING station indices so the growth
/// `access` summation keeps a stable order.
struct StationGrid {
    map: std::collections::BTreeMap<i64, Vec<usize>>,
}
impl StationGrid {
    const CELL: f64 = 1500.0;
    fn key(x: f64, y: f64) -> i64 {
        (x / Self::CELL).floor() as i64 * 73_856_093 + (y / Self::CELL).floor() as i64 * 19_349_663
    }
    fn new(stations: &[crate::types::Station]) -> Self {
        let mut map: std::collections::BTreeMap<i64, Vec<usize>> =
            std::collections::BTreeMap::new();
        for (i, s) in stations.iter().enumerate() {
            map.entry(Self::key(s.pos.x, s.pos.y)).or_default().push(i);
        }
        StationGrid { map }
    }
    fn candidates(&self, p: Vec2) -> Vec<usize> {
        let cx = (p.x / Self::CELL).floor() as i64;
        let cy = (p.y / Self::CELL).floor() as i64;
        let mut out: Vec<usize> = Vec::new();
        for oy in -1..=1 {
            for ox in -1..=1 {
                let k = (cx + ox) * 73_856_093 + (cy + oy) * 19_349_663;
                if let Some(arr) = self.map.get(&k) {
                    out.extend_from_slice(arr);
                }
            }
        }
        out.sort_unstable();
        out
    }
    fn any_within(
        &self,
        p: Vec2,
        stations: &[crate::types::Station],
        radius_for: impl Fn(&crate::types::Station) -> f64,
    ) -> bool {
        let cx = (p.x / Self::CELL).floor() as i64;
        let cy = (p.y / Self::CELL).floor() as i64;
        for oy in -1..=1 {
            for ox in -1..=1 {
                let k = (cx + ox) * 73_856_093 + (cy + oy) * 19_349_663;
                if let Some(arr) = self.map.get(&k) {
                    for &i in arr {
                        let s = &stations[i];
                        if dist(p, s.pos) <= radius_for(s) {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}

/// Run the assignment and thread its outputs back into route/station/flow state
/// and city stats. Mirrors `refreshAssignment`.
pub fn refresh_assignment(state: &mut GameState) {
    let result = run_assignment(state);
    state.flows = result.flows.clone();

    // v0.9 keystone: reliability -> ridership feedback applied AFTER assignment.
    let transit_lost =
        ops::apply_reliability_demand(state, &result.route_ridership, &result.route_revenue);
    state.stats.daily_transit_trips = (result.daily_transit_trips - transit_lost).max(0.0);
    state.stats.daily_car_trips = result.daily_car_trips + transit_lost;
    let total = state.stats.daily_transit_trips + state.stats.daily_car_trips;
    state.stats.transit_share = if total > 0.0 {
        state.stats.daily_transit_trips / total
    } else {
        0.0
    };

    // refresh the surface-grade density cache on surface tracks.
    let mut grade_by_id: std::collections::BTreeMap<u32, crate::types::TrackGrade> =
        std::collections::BTreeMap::new();
    for t in state.tracks.iter_mut() {
        grade_by_id.insert(t.id, t.grade);
        if t.grade == crate::types::TrackGrade::Surface {
            t.congestion_density = Some(segment_density01(Some(&state.fields), t));
        }
    }
    // snapshot tracks (id -> (grade, length, density)) for per-route grade speed.
    let track_info: std::collections::BTreeMap<u32, (crate::types::TrackGrade, f64, f64)> = state
        .tracks
        .iter()
        .map(|t| {
            let dens = if t.grade == crate::types::TrackGrade::Surface {
                t.congestion_density
                    .unwrap_or_else(|| segment_density01(Some(&state.fields), t))
            } else {
                0.0
            };
            (t.id, (t.grade, t.polyline.length, dens))
        })
        .collect();

    for r in state.routes.iter_mut() {
        let cfg = modes(r.mode);
        r.capacity = if r.vehicle_count > 0 {
            (cfg.vehicle_capacity * 3600.0) / r.headway_seconds
        } else {
            0.0
        };
        r.load = r.daily_ridership * PEAK_HOUR_FRACTION;
        r.crowding = if r.capacity > 0.0 {
            r.load / r.capacity
        } else if r.load > 0.0 {
            2.0
        } else {
            0.0
        };
        // per-segment load aligned to segment_ids (segment i joins stop i, i+1).
        r.segment_loads = (0..r.segment_ids.len())
            .map(|i| {
                let a = r.station_ids[i];
                let b = r.station_ids[i + 1];
                result
                    .segment_load
                    .get(&(r.id, a.min(b), a.max(b)))
                    .copied()
                    .unwrap_or(0.0)
            })
            .collect();
        // surface exposure (fraction of segments not in tunnel).
        if !r.segment_ids.is_empty() {
            let exposed = r
                .segment_ids
                .iter()
                .filter(|sid| grade_by_id.get(sid) != Some(&crate::types::TrackGrade::Tunnel))
                .count();
            r.surface_exposure = Some(exposed as f64 / r.segment_ids.len() as f64);
        } else {
            r.surface_exposure = Some(1.0);
        }
        // length-weighted day-average grade speed.
        let mut total_len = 0.0;
        let mut speed_len = 0.0;
        for sid in &r.segment_ids {
            let Some(&(grade, len, dens)) = track_info.get(sid) else {
                continue;
            };
            if len <= 0.0 {
                continue;
            }
            speed_len += segment_day_average_speed_mps(r.mode, grade, dens) * len;
            total_len += len;
        }
        r.move_grade_speed = Some(if total_len > 0.0 {
            speed_len / total_len
        } else {
            cfg.speed
        });
    }

    for s in state.stations.iter_mut() {
        let target = result.station_boardings.get(&s.id).copied().unwrap_or(0.0);
        s.ridership = s.ridership * 0.5 + target * 0.5;
        let alight = result.station_alightings.get(&s.id).copied().unwrap_or(0.0);
        s.alightings = s.alightings * 0.5 + alight * 0.5;
    }

    state.unserved = Some(result.unserved.clone());
    analytics::capture_assignment_analytics(
        state,
        &result.station_boardings,
        &result.station_alightings,
        &result.flows,
        &result.car_flows,
    );

    // coverage: fraction of population within walk radius of any station.
    let cov_grid = StationGrid::new(&state.stations);
    let g = &state.fields;
    let mut covered = 0.0;
    let mut total_pop = 0.0;
    for i in 0..g.population.len() {
        let pop = g.population[i] as f64;
        if pop <= 0.0 {
            continue;
        }
        total_pop += pop;
        let c = cell_center(g, i);
        if cov_grid.any_within(c, &state.stations, |s| modes(s.mode).walk_radius) {
            covered += pop;
        }
    }
    state.stats.coverage = if total_pop > 0.0 {
        covered / total_pop
    } else {
        0.0
    };

    // congestion overlay, scaled by the diurnal demand curve.
    let car_flows: Vec<CarFlow> = result.car_flows.clone();
    let traffic = compute_traffic(state, &car_flows, diurnal_demand(state.tick));
    state.traffic = Some(traffic);
}

// ── daily economy ─────────────────────────────────────────────────────────────

fn run_daily_economy(state: &mut GameState, events: &mut TickEvents) {
    let mut fares = 0.0;
    let mut operations = 0.0;
    let mut maintenance = 0.0;
    for r in &state.routes {
        fares += r.daily_revenue;
        operations += route_operating_cost(r.mode, r.vehicle_count);
    }
    fares *= event_fare_mult(&state.active_events);
    for t in &state.tracks {
        maintenance += (t.polyline.length / 1000.0)
            * modes(t.mode).maint_per_km_per_day
            * grade_maint_mult(t.grade);
    }
    for s in &state.stations {
        maintenance += modes(s.mode).station_cost * 0.0002 * s.level as f64;
    }
    // v0.9 ops opex: fleet maintenance per active unit + standing depot cost.
    maintenance += ops::ops_daily_opex(state);
    // subsidy: base scaled by approval (0.5x..1.5x), declining 2%/year.
    let year = (state.tick / TICKS_PER_DAY as u64 / 365) as i32;
    let base_sub = state
        .scenario_rules
        .as_ref()
        .and_then(|r| r.daily_subsidy)
        .unwrap_or_else(|| base_daily_subsidy(state.difficulty));
    let base = base_sub * 0.98f64.powi(year);
    let subsidy = base * (0.5 + state.stats.approval / 100.0);
    let interest = (state.budget.loan_balance * state.budget.loan_rate) / 365.0;

    let net = fares + subsidy - operations - maintenance - interest;
    let b = &mut state.budget;
    b.cash += net;
    b.last_day = crate::types::DayLedger {
        fares,
        subsidy,
        operations,
        maintenance,
        interest,
    };
    b.net_history.push(net);
    if b.net_history.len() > 7 {
        b.net_history.remove(0);
    }
    let life = b
        .lifetime
        .get_or_insert_with(crate::types::LifetimeLedger::default);
    life.fares += fares;
    life.subsidy += subsidy;
    life.operations += operations;
    life.maintenance += maintenance;
    life.interest += interest;
    life.days += 1;

    if fares > 0.0 && fares > operations + maintenance {
        events
            .messages
            .push("Farebox recovery above 100% - the network pays for itself".to_string());
    }
}

// ── approval ──────────────────────────────────────────────────────────────────

fn update_approval(state: &mut GameState) {
    let mut crowd_riders = 0.0;
    let mut total_riders = 0.0;
    for r in &state.routes {
        total_riders += r.daily_ridership;
        if r.crowding > CROWD_APPROVAL_THRESHOLD {
            crowd_riders += r.daily_ridership * (r.crowding - CROWD_APPROVAL_THRESHOLD);
        }
    }
    let crowd_drag = if total_riders > 0.0 {
        (20.0f64).min((crowd_riders / total_riders) * 40.0)
    } else {
        0.0
    };
    let reliability_delta = ops::ops_approval_delta(state);
    let s = &mut state.stats;
    let target = (25.0
        + s.coverage * 90.0
        + s.transit_share * 60.0
        + event_approval_delta(&state.active_events) * 2.0
        - crowd_drag
        + reliability_delta)
        .clamp(0.0, 100.0);
    s.approval += (target - s.approval) * 0.08;
    s.approval = s.approval.clamp(0.0, 100.0);
}

// ── events ────────────────────────────────────────────────────────────────────

fn update_events(state: &mut GameState, day: i64, events: &mut TickEvents) {
    let mut still: Vec<ActiveEvent> = Vec::new();
    for a in &state.active_events {
        let mut a = a.clone();
        a.days_left -= 1;
        if a.days_left > 0 {
            still.push(a);
        } else if let Some(d) = event_by_id(&a.id) {
            events.toasts.push(Toast {
                message: format!("{} has ended.", d.name),
                tone: ToastTone::Info,
            });
        }
    }
    state.active_events = still;

    let mut rng = Rng::from_state(state.rng_state);
    if state.active_events.is_empty() && day >= state.next_event_day as i64 && rng.chance(0.2) {
        let def = roll_event(rng.next_f64());
        state.active_events.push(ActiveEvent {
            id: def.id.to_string(),
            days_left: def.days,
        });
        state.demand_dirty = true;
        events.toasts.push(Toast {
            message: format!("{} - {}", def.name, def.desc),
            tone: match def.tone {
                EventTone::Good => ToastTone::Good,
                EventTone::Warn => ToastTone::Warn,
                EventTone::Info => ToastTone::Info,
            },
        });
        state.next_event_day = (day + def.days as i64 + 12 + rng.int(0, 10)) as u32;
    }
    state.rng_state = rng.state();
}

// ── unlocks ───────────────────────────────────────────────────────────────────

fn check_unlocks(state: &mut GameState, events: &mut TickEvents) {
    if state.scenario_rules.as_ref().and_then(|r| r.lock_modes) == Some(true) {
        return;
    }
    for mode in [TransitMode::Tram, TransitMode::Metro, TransitMode::Rail] {
        if state.unlocked_modes.contains(&mode) {
            continue;
        }
        if !mode_unlock_ready(mode, &state.stats) {
            continue;
        }
        state.unlocked_modes.push(mode);
        events.mode_unlocked = Some(modes(mode).label.to_string());
        events.messages.push(format!(
            "{} unlocked - your network earned it",
            modes(mode).label
        ));
    }
}

// ── growth ────────────────────────────────────────────────────────────────────

/// Overcrowding-based reliability proxy for growth. Mirrors `networkReliability`.
fn network_reliability(state: &GameState) -> f64 {
    let mut load_w = 0.0;
    let mut penalty = 0.0;
    for r in &state.routes {
        if r.vehicle_count == 0 {
            continue;
        }
        let w = r.daily_ridership.max(1.0);
        load_w += w;
        penalty += w * (r.crowding - 1.0).clamp(0.0, 1.0);
    }
    let overload = if load_w > 0.0 { penalty / load_w } else { 0.0 };
    1.1 - 0.5 * overload
}

/// Weekly growth pass. Mirrors `runGrowth`.
fn run_growth(state: &mut GameState) {
    let growth_grid = StationGrid::new(&state.stations);
    let rel = network_reliability(state);
    let mut rng = Rng::from_state(state.rng_state);
    let mut total_pop = 0.0;
    let n = state.fields.population.len();
    for i in 0..n {
        let pop = state.fields.population[i] as f64;
        if state.fields.water[i] == 1 {
            continue;
        }
        let c = cell_center(&state.fields, i);
        let mut access = 0.0;
        for si in growth_grid.candidates(c) {
            let s = &state.stations[si];
            let d = dist(c, s.pos);
            let walk_r = modes(s.mode).walk_radius;
            if d < walk_r * 1.5 {
                access += (s.level as f64 * (walk_r / d.max(50.0)).min(1.0))
                    * (1.0 + s.ridership / 5000.0);
            }
        }
        if access > 0.5 && pop > 5.0 {
            let growth = (0.03f64).min(0.004 * access * rel) * (0.8 + rng.next_f64() * 0.4);
            state.fields.population[i] = (pop * (1.0 + growth)) as f32;
            let lv = state.fields.land_value[i] as f64;
            state.fields.land_value[i] = (3.0f64).min(lv * (1.0 + growth * 0.5)) as f32;
            let jobs = state.fields.jobs[i] as f64;
            state.fields.jobs[i] = (jobs * (1.0 + growth * 0.6)) as f32;
        } else if access == 0.0 && pop > 5.0 {
            state.fields.population[i] = (pop * 0.9995) as f32;
        }
        total_pop += state.fields.population[i] as f64;
    }
    state.rng_state = rng.state();

    // refresh district aggregates + per-district growth delta.
    for d in state.districts.iter_mut() {
        let mut pop = 0.0;
        let mut jobs = 0.0;
        for &i in &d.cell_indices {
            pop += state.fields.population[i as usize] as f64;
            jobs += state.fields.jobs[i as usize] as f64;
        }
        let prev = d.population;
        d.last_growth_delta = Some(if prev > 0.0 { (pop - prev) / prev } else { 0.0 });
        d.population = pop;
        d.jobs = jobs;
    }
    state.stats.population = total_pop;
    state.stats.jobs = state.districts.iter().map(|d| d.jobs).sum();
    state.demand_dirty = true;
}
